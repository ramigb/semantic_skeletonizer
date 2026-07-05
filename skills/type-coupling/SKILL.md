---
name: type-coupling
description: Classify import edges as runtime vs type-only coupling to plan safe refactors and module extractions. Use when the user asks "how tightly coupled are these modules", "can I extract this into a package", "what's safe to move", or plans a modularization.
---

# Type vs Runtime Coupling

Grade every edge in the semantic-skeletonizer graph by *strength*. Type-only
imports are erased at compile time — they're weak coupling you can cut or
cross freely. Runtime imports are the real structure. This distinction is
what makes extraction plans realistic instead of hand-wavy.

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server. Per file, join `import_records`
(`source`, `names`, `type_only`) with `dependencies` (resolved keys) —
match record to resolved key via the record's relative `source`.

## Edge grading

For each resolved internal edge A → B:

- **type-only**: every record from A into B has `type_only: true`, or all
  imported names resolve to `interface`/`type` kind symbols in B's symbol
  table (a value-position import of only types is still weak in practice —
  grade it "type-ish" and note the `import type` cleanup opportunity).
- **narrow runtime**: 1–2 runtime names imported.
- **broad runtime**: 3+ names or a namespace import.

## Uses

**Coupling report** (default): per module pair, the edge grade mix.

```
## Coupling: src/billing/ ↔ rest of codebase
inbound:  9 edges — 6 type-only, 2 narrow, 1 broad
outbound: 4 edges — 4 narrow (all into src/utils/)

Verdict: billing is 1 broad edge away from extractable.
The broad edge: components/Invoice.tsx imports {calc, format, tax, rates}
from billing/engine.ts → give billing a public index.ts facade first.
```

**Extraction feasibility** (when the user names a candidate module): count
the edges that would become package-boundary crossings; type-only ones are
free (published types), runtime ones each need a decision (move, invert, or
accept the dependency). Output a concrete step list.

**`import type` hygiene** (bonus, offer it): records with `type_only: false`
whose names are all type-kind symbols → should be `import type` (better
tree-shaking, breaks fake runtime cycles). List file + line-ready fix.

## Caveats

- Symbol-kind lookup fails for renamed re-exports; grade those edges
  conservatively as runtime.
- A type-only edge still couples *shapes* — API changes in B's types break
  A at compile time. "Weak" means no runtime/bundle entanglement, not zero
  cost; say so in the verdict.
