---
name: blast-radius
description: Impact analysis before changing a file — who imports it, transitively, and what's at risk. Use when the user asks "what breaks if I change X", "what depends on this file", "is it safe to refactor X", or wants a review briefing for a diff/PR touching specific files.
---

# Blast Radius

Compute the transitive set of files affected by changing one or more target
files, using the semantic-skeletonizer MCP server's resolved import graph.

## Getting the data

1. Read the MCP resource `skeleton://project/global` from the
   `semantic-skeletonizer` server. It returns a JSON object keyed by
   repo-relative file paths; each node has `dependencies` (files it imports)
   and `symbols`.
2. For a single file, `get_dependencies {"file_path": "...", "direction": "in"}`
   returns `imported_by` directly — but for *transitive* analysis, fetch the
   global graph once and traverse it yourself instead of many tool calls.
3. If the graph is empty, the server may still be sweeping a large repo —
   wait a moment and re-read.

## Algorithm

1. Build the reverse adjacency: for every file F and every entry D in
   `F.dependencies`, record F as a dependent of D.
2. From each target file, BFS over reverse edges. Record the depth at which
   each file is first reached (depth 1 = direct importers).
3. For depth-1 files, identify *which* symbols they pull in: their
   `import_records` entries whose `source` resolves to the target — the
   `names` list is what they actually use. Flag `type_only: true` records as
   low-risk (type-level coupling only; no runtime behavior can break).
4. If the user gave a symbol (not just a file), narrow depth-1 to importers
   whose `import_records.names` include that symbol; the deeper closure is
   then an upper bound, and say so.

## Report format

Lead with the numbers, then the tree:

```
## Blast radius: src/utils/api.ts
Direct importers: 3 · transitive: 11 of 84 files (13%)

src/utils/api.ts
├─ src/components/Form.tsx        (uses: validateUser)          [runtime]
│  └─ src/pages/Signup.tsx
├─ src/components/Login.tsx       (uses: validateUser, login)   [runtime]
└─ src/types/session.ts           (uses: User)                  [type-only ✓]
```

- Sort siblings by their own dependent count (riskiest first).
- Call out affected *test* files (`*.test.*`, `*.spec.*`) separately — they
  tell the user what to run.
- Close with a one-paragraph risk assessment: how contained the change is,
  which downstream file looks most fragile, and whether most coupling is
  type-only.

## Caveats to state honestly

- Edges are **file-level imports**, not a call graph. A file that imports the
  target but doesn't call the changed function is a false positive; say the
  numbers are an upper bound.
- Dynamic `import()` with non-literal paths and tsconfig path aliases are not
  resolved and will be missing.
