# Agent Skills for Semantic Skeletonizer

Eleven ready-made analysis skills that turn the MCP server's graph into reports.
The server stays a fast data plane (skeletons + resolved import edges); the
intelligence lives here, in prompts the agent picks up.

| Skill | Question it answers |
|---|---|
| [blast-radius](blast-radius/SKILL.md) | What breaks if I change this file? |
| [circular-deps](circular-deps/SKILL.md) | Where are the import cycles, and where do I cut them? |
| [coupling-hotspots](coupling-hotspots/SKILL.md) | Which files are god-files / refactor bottlenecks? |
| [architecture-map](architecture-map/SKILL.md) | Draw the architecture, flag layer violations |
| [dead-exports](dead-exports/SKILL.md) | What exported code has no consumers? |
| [api-docs](api-docs/SKILL.md) | Generate always-current API / architecture docs |
| [component-inventory](component-inventory/SKILL.md) | What React components exist, with what props, composed how? |
| [untested-exports](untested-exports/SKILL.md) | Which exports never reach a test? |
| [dependency-audit](dependency-audit/SKILL.md) | Which npm packages are used where; what can go? |
| [type-coupling](type-coupling/SKILL.md) | How tightly coupled are modules — what's safe to extract? |
| [codebase-tour](codebase-tour/SKILL.md) | Where do I start reading this codebase? |

## Install

The skills go into the **project you're analyzing** (the one the MCP server
watches), not this repo:

```bash
# Claude Code (project-level)
mkdir -p /path/to/your/project/.claude/skills
cp -r skills/* /path/to/your/project/.claude/skills/

# or user-level, available in every project
cp -r skills/* ~/.claude/skills/
```

Each skill assumes the `semantic-skeletonizer` MCP server is connected and
fetches its data from the `skeleton://project/global` resource plus the
server's tools (`get_dependencies`, `list_symbols`, `search_symbols`,
`get_implementation`).

Skills trigger automatically when a request matches their description
(e.g. "what breaks if I change api.ts?" → blast-radius), or explicitly via
`/blast-radius` style invocation in clients that support it.

## Shared ground rules baked into every skill

- **One graph read, then compute.** Fetch `skeleton://project/global` once
  and traverse locally instead of hammering per-file tools.
- **File-level honesty.** Edges are resolved imports, not a call graph;
  every skill states this where it changes conclusions.
- **Candidates, not verdicts.** Anything destructive (dead code, removable
  deps) is reported as a candidate list with a verification step.
