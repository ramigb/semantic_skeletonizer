---
name: coupling-hotspots
description: Rank files by coupling (fan-in/fan-out), find god-files and refactor candidates. Use when the user asks "which files are doing too much", "where should I start refactoring", "what are the riskiest files", or wants code-health metrics.
---

# Coupling Hotspots

Compute fan-in/fan-out metrics from the semantic-skeletonizer MCP graph and
rank the files most likely to hurt.

## Getting the data

Read the MCP resource `skeleton://project/global` from the
`semantic-skeletonizer` server. Per file: `dependencies` (fan-out edges),
`symbols` (size proxy: count + kinds). Build fan-in by reversing the edges.

## Metrics (per file)

- **Ca (afferent / fan-in):** files importing it.
- **Ce (efferent / fan-out):** files it imports (`dependencies.length`).
- **Instability I = Ce / (Ca + Ce)** — 0 = stable (everyone depends on it),
  1 = unstable (depends on everything, nothing depends on it).
- **Symbol count** from `symbols.length` — a rough size/responsibility proxy.

## What to flag

1. **God files:** high Ca AND high Ce (top quartile in both). Everything
   depends on them and they depend on everything — the classic refactor
   bottleneck. Suggest what to extract: look at their `symbols` list — a file
   whose symbols span many kinds (components + utils + types) is begging to
   be split along those lines.
2. **Unstable dependencies of stable files:** an edge from low-I to high-I
   file violates the stable-dependencies principle; churn in the unstable
   file ripples into the stable one.
3. **Hub types:** files that are ~all `interface`/`type` symbols with huge
   fan-in are fine (that's what shared type modules are for) — exclude them
   from god-file flagging, mention them as healthy hubs.

## Report format

```
## Coupling hotspots (84 files, 212 edges)

| file | fan-in | fan-out | I | symbols | verdict |
|---|---|---|---|---|---|
| src/utils/helpers.ts | 31 | 14 | 0.31 | 42 | ⚠ god file — split |
| src/types.ts | 40 | 0 | 0.00 | 25 | ✓ healthy type hub |
```

Table capped at ~10 rows (worst first), then a prose paragraph per flagged
file: *why* it's flagged and one concrete, named split suggestion (e.g.
"symbols `formatDate`, `parseDate`, `toUTC` form a date cluster → extract
`src/utils/date.ts`"). Skip metrics lectures; verdicts only.

## Caveats

- File-level edges: a 1-symbol import and a 20-symbol import both count as
  one edge. Use `import_records.names` lengths to weight edges if the user
  wants precision.
- Small repos (< ~20 files) make quartile-based flags meaningless — fall back
  to absolute judgment and say so.
