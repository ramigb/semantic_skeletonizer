---
name: circular-deps
description: Detect circular import chains in the codebase and explain how to break them. Use when the user asks about circular dependencies, import cycles, "why is this import undefined at runtime", or wants a dependency health check.
---

# Circular Dependencies

Find every import cycle in the project using the semantic-skeletonizer MCP
server's resolved edges, and propose the cheapest place to cut each one.

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server once. Each node's `dependencies` array is the
outgoing edge list (repo-relative keys). If empty, the server may still be
sweeping — retry once.

## Algorithm

1. Run Tarjan's SCC (or iterative DFS with a color/stack set — recursion
   depth can exceed limits on big repos) over the `dependencies` adjacency.
2. Every strongly-connected component with more than one node — or a
   self-loop — is a cycle cluster. Within each cluster, extract one or two
   representative cycles (shortest first) for display; don't enumerate all
   paths in a large SCC.
3. Grade each cycle edge by weight, using `import_records` of the importing
   file:
   - `type_only: true` edge → **trivially breakable** (types can move to a
     shared `types.ts`, or the import can stay — TS erases it at runtime).
   - Edge importing 1 symbol → cheap to break (move that symbol).
   - Edge importing many symbols → expensive; probably the "intended"
     direction.
4. Recommend cutting the *cheapest* edge of each cycle, and say where the
   moved symbol(s) could live.

## Report format

```
## Import cycles: 2 found (checked 84 files)

### Cycle 1 (3 files)
src/models/user.ts → src/services/auth.ts → src/models/session.ts → src/models/user.ts
Cheapest cut: session.ts → user.ts imports only the `User` type (type-only).
Fix: none needed at runtime, or move `User` to src/models/types.ts.

### Cycle 2 (2 files) ⚠ runtime
src/a.ts ⇄ src/b.ts — mutual runtime imports (a uses initB; b uses configA)
Fix: extract configA into src/config.ts; both sides import downward.
```

If no cycles: say so in one line and show the 3 longest dependency *chains*
instead (they indicate depth, the usual precursor to cycles).

## Caveats

- Runtime cycles cause real bugs (partially-initialized modules, `undefined`
  bindings); type-only cycles are harmless in TS. Always separate the two.
- tsconfig path aliases and dynamic imports are not resolved — a cycle routed
  through an alias will be invisible.
