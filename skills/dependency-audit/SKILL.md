---
name: dependency-audit
description: Audit external package usage — where each npm package is imported, single-use deps, type-only deps that belong in devDependencies. Use when the user asks "what packages do we use where", wants to slim dependencies, or is planning an upgrade/removal.
---

# External Dependency Audit

Map every npm package to the exact files importing it, using the
semantic-skeletonizer graph's `external_deps` and `import_records`.

## Getting the data

1. Read the MCP resource `skeleton://project/global` from the
   `semantic-skeletonizer` server. Per file: `external_deps` (package names,
   `@scope/name`-aware) and `import_records` (which names, `type_only`).
2. Read `package.json` for `dependencies`/`devDependencies` to compare
   declared vs actually-imported.

## Analysis

Build `package → [files]` and per-package aggregates, then look for:

1. **Spread:** how many files import each package. High spread = expensive to
   ever remove or swap (e.g. `react` everywhere — fine, expected). Low spread
   on a heavy package = cheap win: a wrapper module or removal.
2. **Single-use packages:** imported by exactly one file. Classic candidates
   for inlining (left-pad tier) or at least isolation.
3. **Type-only packages:** every import record for the package has
   `type_only: true` → runtime never needs it; it belongs in
   `devDependencies` (bundle size + install surface win).
4. **Declared but never imported:** in `package.json` deps but absent from
   all `external_deps` → possibly removable (check scripts/configs first —
   build plugins are invoked, not imported).
5. **Imported but undeclared:** in `external_deps` but not in `package.json`
   → works via transitive install today, breaks tomorrow. Flag loudly.
6. **Deep imports** (`lodash/get`, `date-fns/format`): note them — they pin
   internal package layout and complicate major-version upgrades.

## Report format

```
## Dependency audit — 14 packages across 84 files

| package | files | names imported | flags |
|---|---|---|---|
| react | 41 | — | core |
| date-fns | 2 | format, parseISO | deep imports |
| moment | 1 | moment | single-use → replace with date-fns (already present) |
| @types/node-ish | 3 | types only | → devDependencies |

⚠ Undeclared but imported: `classnames` (src/components/Tag.tsx)
Unimported but declared: `lodash` — check build scripts before removing.
```

Finish with a short ranked action list (move X to devDeps, remove Y, wrap Z)
with the expected benefit of each.

## Caveats

- The graph only sees static `import` statements in `.ts/.tsx`:
  `require()` in JS config files, CLI usage, and build-tool plugins are
  invisible — always caveat "declared but never imported" findings.
- Version and size info isn't in the graph; if the user wants size impact,
  check `node_modules` or a bundle report separately.
