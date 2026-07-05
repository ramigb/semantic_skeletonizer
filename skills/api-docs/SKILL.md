---
name: api-docs
description: Generate always-current API/module documentation from exported symbols, signatures, and preserved JSDoc. Use when the user asks to "document the codebase", "generate API docs", write/refresh ARCHITECTURE.md, or produce an onboarding reference.
---

# API Docs Generator

Produce module-by-module reference docs from the semantic-skeletonizer graph.
Everything needed is already extracted: exported symbols, one-line signatures,
full skeleton text, and JSDoc blocks (the server preserves `/** ... */` on
top-level declarations).

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server. Per file: `exports` (skeleton text incl.
JSDoc), `symbols` (name/kind/exported/signature), `dependencies`,
`external_deps`. For a symbol that deserves deeper documentation, fetch its
real body with the `get_implementation` tool — sparingly.

## Structure

One section per *module* (directory with source files), ordered by fan-in
(most-depended-on first — that's what readers need first):

```markdown
# API Reference
_Generated from the live semantic graph — regenerate anytime; do not edit by hand._

## src/utils/ — core helpers (imported by 23 files)

### `validateUser(u: string): boolean`  ·  function · api.ts
Validates a user id against the registry.   <!-- from JSDoc -->

### `UserService` · class · api.ts
Methods: `getUser(id: string): string`, `static fromEnv(): UserService`
```

Rules:
- **Exported symbols only.** Internal helpers are noise in an API doc.
- Use the JSDoc sentence when present (it's at the top of the matching
  `exports` entry); otherwise derive one neutral sentence from the name and
  signature — never invent behavior claims you can't see.
- Group class methods under their class; show `interface`/`type` bodies from
  the skeleton text (they're complete by design), collapsed to ≤ 10 lines.
- Per module, add one "used by" line (top 3 dependents) so readers see
  context and coupling.
- Note elided constants as e.g. `BIG_TABLE: Item[] (large literal, 5.6 KB)`.

## Modes

- **`API.md`** (default): the reference above.
- **`ARCHITECTURE.md`**: shorter — per-module role paragraphs + the module
  dependency ordering, no per-symbol entries. Pairs well with the
  architecture-map skill's Mermaid diagram; include it if the user wants one
  file.
- **Single module**: same format, one section, when the user names a
  directory.

Write the file at repo root unless told otherwise, and echo it in the reply.

## Caveats

- JSDoc coverage is whatever the code has — report the coverage ratio
  ("31/78 exported symbols documented") so gaps are visible.
- Docs describe *interfaces*, not behavior. For behavior-critical symbols,
  read the implementation before writing anything beyond the signature.
