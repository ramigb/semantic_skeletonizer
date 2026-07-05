---
name: dead-exports
description: Find exported symbols nothing imports, and orphan files nothing depends on. Use when the user asks about dead code, unused exports, "what can I delete", or wants a cleanup pass.
---

# Dead Exports

Cross-reference every exported symbol against every import in the project to
find code with no consumers — a query the semantic-skeletonizer graph answers
precisely where grep guesses.

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server. Needed per file: `symbols` (with `exported`
flags), `import_records` (`source`, `names`), and `dependencies` (resolved
targets, index-aligned intent with `import_records` — match a record to its
resolved file by re-checking `source` against the target's path).

## Algorithm

1. **Collect the demand side:** for every file, for every import record,
   resolve which project file it refers to (the record's `source` is
   relative; the file's `dependencies` holds the resolved keys — match by
   suffix). Accumulate `used[target_file] ⊇ names`. Treat namespace imports
   (`* as x`) and `"*"` re-export records as *using everything* from the
   target — mark that target fully-used.
2. **Collect the supply side:** every symbol with `exported: true`. For
   classes, only the class name itself is importable (skip `Class.method`
   entries).
3. **Dead export** = exported symbol whose name never appears in the union of
   its file's users' name sets (and the file isn't fully-used).
4. **Orphan file** = file with zero dependents that also exports something.
5. **Exempt entrypoints** before reporting: `index.ts(x)` at package roots,
   `main/app/cli` files, framework-convention paths (`pages/`, `app/`,
   `routes/` — imported by the framework, not by code), config files, test
   files, and anything matching `package.json`'s `main`/`exports`/`bin` if
   readable. List exemptions applied.

## Report format

```
## Dead code report (84 files scanned)

### Dead exports — 7 candidates
| symbol | kind | file | note |
|---|---|---|---|
| formatLegacy | function | src/utils/format.ts | no importer names it |
| OldProps | interface | src/components/Old.tsx | file itself is orphaned |

### Orphan files — 2
- src/components/Old.tsx (exports 3 symbols, 0 dependents)

Exempted as entrypoints: src/index.ts, pages/* (7 files)
```

End with suggested next step: verify each with a project-wide text search
(dynamic access, string-keyed registries, and reflection escape the graph),
then delete in one commit per cluster. Offer to run those greps.

## Caveats — always state these

- Findings are **candidates, not verdicts**: dynamic `import()`, re-export
  renames (`export { a as b }`), string-based lookups, and framework magic
  all evade the graph. Never delete without the grep confirmation pass.
- Public library packages export things for *external* consumers — if
  `package.json` has real `exports`, the whole analysis only applies to
  internal modules.
