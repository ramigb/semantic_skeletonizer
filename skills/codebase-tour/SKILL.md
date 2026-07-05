---
name: codebase-tour
description: Generate a guided reading order for a codebase — where to start, what each stop is, in dependency order. Use when the user is new to a repo, asks "where do I start reading", "give me a tour", "explain how this codebase fits together", or onboards someone.
---

# Codebase Tour

Write a "read the code in this order" guide from the semantic-skeletonizer
graph: start at the foundations everything depends on, end at the
entrypoints, and say what to look for at each stop.

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server. Use `dependencies` for ordering, `symbols`
for describing stops, and the `get_implementation` tool to pull 1–3 short
key bodies where seeing real code makes the tour concrete (an entrypoint's
main function, the core domain type's file).

## Building the tour

1. **Identify the poles.** Entrypoints: files with zero dependents (after
   exempting tests/configs) or framework-convention paths (`pages/`,
   `main.ts`, `index.ts` at root). Foundations: highest fan-in files —
   usually types and core utils.
2. **Order stops bottom-up:** topological order over `dependencies`
   (foundations first), but *curated* — group files into 5–9 stops by module,
   not one stop per file. Break cycles arbitrarily and move on.
3. **Describe each stop from evidence:** the module's exported symbols and
   signatures tell you what it *is*; its dependents tell you why it matters.
   Name the 2–3 symbols worth actually reading, with one line each on what
   the signature reveals.
4. **End at an entrypoint walk-through:** pick the main entrypoint and trace
   one request/render path downward through the stops the reader now knows,
   so the tour closes the loop.

## Output format

```markdown
# Codebase tour — myapp (84 files)
Suggested time: ~45 min · Path: types → utils → services → components → pages

## Stop 1 · src/types/ — the domain vocabulary (imported by 40 files)
Read `User`, `Session`, `Order` in types/domain.ts. Everything else speaks
these shapes; the rest of the tour assumes them.

## Stop 2 · src/utils/api.ts — the backend boundary (18 dependents)
`validateUser(u): boolean`, `UserService` — every server interaction funnels
through here. Note: nothing above this layer touches fetch directly.
...
## Finale · pages/Signup.tsx — one flow, top to bottom
Signup → Form → validateUser → UserService: you've now read every layer this
path crosses.
```

Keep it a document (offer to save as `TOUR.md`), one short paragraph per
stop. Confidence rule: descriptions must come from symbols/signatures/JSDoc
actually in the graph — where intent is unclear, say "appears to" or read
the implementation before asserting.

## Caveats

- Zero-dependent files may be dead code rather than entrypoints —
  cross-check with framework conventions before crowning one.
- The graph covers `.ts/.tsx` only; if key logic lives in `.js`, config, or
  the backend, note the blind spot at the start of the tour.
