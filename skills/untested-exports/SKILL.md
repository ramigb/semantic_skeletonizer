---
name: untested-exports
description: Map which exported symbols have no test importing them. Use when the user asks "what's untested", "where are the coverage gaps", "which modules lack tests", or before hardening/refactoring work.
---

# Untested Exports

Structural test-coverage mapping: which exported symbols are imported by at
least one test file, and which by none. Faster than instrumentation and works
without running anything — but it measures *imported by a test*, not
*asserted on*, and the report must say so.

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server. Test files are graph keys matching
`*.test.ts(x)`, `*.spec.ts(x)`, or living under `__tests__/`, `tests/`,
`test/`. Everything else is production code.

## Algorithm

1. For every test file, take its `dependencies` (resolved production files it
   imports) and the matching `import_records.names` — those names are the
   symbols under test.
2. For every production file, list symbols with `exported: true`
   (skip `Class.method` entries; the class import covers them, though a test
   importing the class doesn't necessarily test every method — note this).
3. Coverage per symbol: tested (named by ≥1 test import, or its file is
   namespace-imported by a test), untested otherwise.
4. Aggregate per file and per directory. Weight by risk: sort untested files
   by their **fan-in** (dependents count) — an untested file that 20 files
   depend on matters more than an untested leaf.

## Report format

```
## Test coverage map (structural) — 61% of exported symbols reach a test

### Highest risk: untested AND heavily depended on
| file | untested symbols | fan-in |
|---|---|---|
| src/utils/format.ts | formatDate, formatMoney (2/2) | 18 |

### Fully untested directories
- src/services/ — 0 of 3 files imported by any test

### Well covered
- src/utils/api.ts — validateUser, UserService ✓ (api.test.ts)
```

Close with the 2–3 test files that would close the most risk (name file +
symbols each should cover), not a generic "write more tests".

## Caveats

- **Import ≠ assertion.** A symbol imported in a test's setup counts as
  tested here. Frame all numbers as an upper bound on coverage.
- E2E tests that drive the app without importing modules make this analysis
  blind to their coverage; ask whether an E2E suite exists.
- If no test files are found at all, just say that and stop — don't produce
  a 100%-untested wall of shame.
