# Video Explainer Brief — Semantic Skeletonizer

> **Instructions for the video generator (NotebookLM):** This document is the
> primary script source for a video overview of the Semantic Skeletonizer
> project. It contains presentation guidance (audience, tone, narrative arc)
> followed by the complete, self-contained facts. Prefer the numbers,
> examples, and phrasings given here — they are measured and accurate. The
> "What NOT to say" section lists claims that would be wrong.

---

## Presentation guidance

- **Audience:** developers who use AI coding assistants (Claude, Codex,
  Cursor) on TypeScript/React projects. They know what an LLM and an import
  statement are; they may not know what MCP or an AST is — define both once,
  briefly.
- **Tone:** practical and honest, engineer-to-engineer. This project's
  identity is *measured claims over hype* — the README even reports a
  benchmark where the tool performs badly. Keep that spirit.
- **Length:** 5–8 minutes works well. Suggested arc below.
- **One-sentence pitch:** *"A tiny Rust server that gives your AI assistant a
  live, token-cheap map of your TypeScript codebase — the shape of every
  file and how they connect, without the implementation bodies."*

## Suggested narrative arc

1. **The problem (60s).** An AI assistant working on a real codebase must
   choose between reading everything (slow, expensive, context overflows) or
   guessing from a few files (wrong). Most of a codebase's tokens are
   implementation details irrelevant to any single question. And any static
   summary goes stale the moment you save a file.
2. **The idea (60s).** Skeletonize: strip every function body, keep every
   signature, type, interface, class shape, JSDoc comment, and import.
   Maintain that skeleton *live* in memory, updated on every file save, and
   serve it to the AI over the Model Context Protocol. The assistant gets an
   aerial map; when it truly needs one function's logic, it asks for exactly
   that function.
3. **Show the skeleton (45s).** Use the before/after example below — a real
   function becoming a one-line signature, a 5.6 KB data table becoming
   `/* elided: 5614 bytes */`.
4. **The graph (60s).** It's not just per-file summaries: imports are
   resolved into edges, so the server knows *which files depend on which* —
   both directions. That enables "what breaks if I change this?" answers.
5. **The measured numbers (45s).** Present the benchmark table including the
   negative result, and explain why it's negative. Honesty is the feature.
6. **Using it (60s).** Build with cargo, add one JSON block to Claude
   Desktop / one TOML block to Codex, done. Show the five tools by example
   question rather than by API.
7. **The skills layer (60s).** The server is deliberately "dumb and fast";
   eleven ready-made agent skills turn its data into reports: blast radius,
   circular dependencies, dead exports, architecture diagrams, a guided
   codebase tour. Analogy: the server is the map data; the skills are the
   navigation apps.
8. **Limits and close (30s).** File-level edges, TypeScript-only, candidates
   not verdicts. Then the pitch sentence again.

---

## The facts

### What it is

Semantic Skeletonizer is a Model Context Protocol (MCP) server written in
Rust. MCP is an open standard that lets AI assistants connect to external
tools and data sources over a simple JSON-RPC connection — the assistant
"plugs in" the server and gains its resources and tools.

The server watches one TypeScript/React project directory. It parses every
`.ts`/`.tsx` file with **oxc** (a very fast Rust-based JavaScript/TypeScript
parser), strips implementation bodies from the syntax tree, and keeps the
resulting "skeletons" in an in-memory graph, keyed by clean repo-relative
paths like `src/utils/api.ts`.

### What a skeleton looks like

Original source (a real function):

```ts
/**
 * Validates a user id against the registry.
 */
export function validateUser(u: string): boolean {
  // reject empty ids
  return u.length > 0;
}
```

In the skeleton this becomes:

```ts
/**
 * Validates a user id against the registry.
 */
export function validateUser(u: string): boolean {}
```

The signature, types, and the JSDoc documentation survive; the body is gone.
Interfaces and type definitions are kept **complete** — they are the highest
signal per token. Large data literals are elided: a 5,614-byte constant
array becomes `const BIG_TABLE: Item[] = /* elided: 5614 bytes */;`.

### It's a live graph, not a snapshot

- A filesystem watcher (the `notify` crate) sees every save. Events are
  debounced (200 ms), filtered through the project's `.gitignore`
  (`node_modules` and `.git` are always skipped), and only the changed file
  is re-parsed.
- Creates, deletes, and renames update the graph correctly; a file that is
  mid-edit and temporarily unparseable keeps its last good skeleton — the
  server never crashes on malformed input.
- Imports are **resolved into edges**: `import { validateUser } from
  '../utils/api'` becomes a graph edge `Form.tsx → api.ts`, and a reverse
  index answers "who imports api.ts?" instantly. Package imports like
  `react` are tracked separately as external dependencies, including whether
  an import is type-only.
- Connected AI clients are notified over MCP (`resources/updated`,
  `list_changed`) the moment the graph changes, so the assistant's map is
  never stale.

### The measured numbers (present the table, including the negative row)

Benchmarked on real open-source repos; tokens counted with tiktoken:

| Repo | Files | Source tokens | Skeleton tokens | Savings |
|---|---:|---:|---:|---:|
| zustand (state library) | 34 | 66,946 | 19,882 | **70.3% fewer** |
| zod (validation library) | 401 | 667,815 | 365,966 | **45.2% fewer** |
| type-fest (type utilities) | 427 | 324,485 | 387,851 | **19.5% MORE** |

Why the negative row: type-fest is almost entirely type declarations — there
are no function bodies to strip, and the JSON envelope adds overhead. The
honest rule: savings scale with how much *implementation* code you have.
Implementation-heavy apps save the most; types-only packages should skip
this tool. The repo ships the benchmark script (`scripts/bench.py`) so
anyone can measure their own codebase.

### The five tools (explain by example question)

| When the assistant wonders... | It calls | And gets |
|---|---|---|
| "What's in this file?" | `list_symbols` | Every top-level symbol: name, kind (function / class / interface / type / component / ...), exported or not, one-line signature |
| "Where is anything called *validate*?" | `search_symbols` | `[{file, name, kind}]` matches across the whole project |
| "Show me the actual code of `validateUser`" | `get_implementation` | That function's **original source text**, byte-exact with comments — not an AST dump, not the whole file |
| "Who depends on this file?" | `get_dependencies` | `imports`, `imported_by`, and external packages for a file |
| "List callable things" | `list_functions` | Back-compat alias of `list_symbols` filtered to callables |

Also notable: `get_implementation` addresses class methods as
`ClassName.methodName` and the default export as `"default"`, and an unknown
name returns a helpful list of the symbols that *do* exist in the file.

### Setting it up

1. Build: `cargo build --release` (Rust toolchain required). Binary:
   `target/release/semantic_skeletonizer`.
2. Register it with your MCP client. Claude Desktop
   (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "semantic-skeletonizer": {
      "command": "/path/to/target/release/semantic_skeletonizer",
      "args": ["--root", "/path/to/your/project"]
    }
  }
}
```

   Codex (`config.toml`):

```toml
[mcp_servers.semantic-skeletonizer]
command = "/path/to/target/release/semantic_skeletonizer"
```

3. Without `--root`, the server maps whatever directory the client launches
   it in — convenient for per-project use. Startup is instant: the server
   answers the MCP handshake in about 2 milliseconds and builds the graph in
   the background, announcing completion with a notification.
4. There's also a small built-in web dashboard (printed to stderr on start)
   showing the live graph, request logs, and server status.

### The skills layer (the "second act" of the project)

Design principle: **the server is a data plane, the intelligence lives in
the agent.** Rather than baking analysis features into Rust, the repo ships
eleven `SKILL.md` prompt files (in `skills/`) that AI agents pick up
automatically. Each skill is a recipe: fetch the graph once, run a specific
algorithm, produce a specific report, state specific caveats.

The eleven skills and the question each answers:

1. **blast-radius** — what breaks if I change this file? (transitive
   impact analysis with depth and risk grading)
2. **circular-deps** — where are the import cycles, and what's the cheapest
   edge to cut?
3. **coupling-hotspots** — which files are god-files? (fan-in/fan-out
   metrics)
4. **architecture-map** — draw the architecture as a Mermaid diagram and
   flag layer violations (a utility importing from the UI layer, etc.)
5. **dead-exports** — which exported symbols does nothing import?
6. **api-docs** — generate always-current API documentation from signatures
   and the preserved JSDoc
7. **component-inventory** — every React component, its props, and the
   composition tree
8. **untested-exports** — which exports never reach a test file?
9. **dependency-audit** — which npm packages are used where; type-only
   dependencies that could move to devDependencies; undeclared imports
10. **type-coupling** — grade module coupling as runtime vs type-only to
    plan safe extractions
11. **codebase-tour** — a guided "read the code in this order" onboarding
    document

Install: copy the skill folders into the analyzed project's
`.claude/skills/` directory. Then a plain question like *"what breaks if I
change api.ts?"* triggers the right skill, which uses the MCP server's data.

### Honest limitations (include these — they build trust)

- **File-level edges, not a call graph.** The graph knows `Form.tsx` imports
  `api.ts`; it does not know `Form` *calls* `validateUser`. Impact analyses
  are upper bounds. (The project explicitly refuses to call itself a "Code
  Property Graph" for this reason.)
- **TypeScript/TSX only.** Plain `.js`, config files, and non-JS languages
  are invisible.
- **Static imports only.** Dynamic `import(variable)` and tsconfig path
  aliases (`@/utils`) are not resolved yet (the resolver has a documented
  extension seam for aliases).
- **Skills report candidates, not verdicts.** Dead-code and
  dependency-removal findings always come with a "verify with a text search
  before deleting" step.

### Technical credibility details (sprinkle, don't dwell)

- Written in Rust on tokio; parsing via oxc, one of the fastest TS parsers
  available. A 400-file repo sweeps in well under a second.
- Robust protocol implementation: version negotiation, subscriptions,
  percent-encoded resource URIs, cancellation-safe stdio handling, graceful
  handling of malformed requests.
- Tested with unit tests plus integration tests that spawn the real binary
  in fixture projects and drive it over stdio, asserting live-update
  behavior end to end.

---

## Key messages (the video should land these)

1. Your AI assistant doesn't need your code — it needs your code's *shape*,
   and the shape is 45–70% cheaper in tokens (measured, reproducible).
2. It's live: save a file and the map updates before your next prompt.
3. It knows the connections: resolved import edges make "what breaks if..."
   answerable.
4. The smarts are skills, not server features — extend it by writing a
   prompt file, not Rust.
5. It tells you when *not* to use it (types-only codebases).

## What NOT to say

- Don't call it a "Code Property Graph" or claim call-graph / control-flow /
  data-flow analysis — it has none of those.
- Don't claim "80–90% token savings" — that was an old unmeasured figure the
  project deliberately replaced with measured 45–70% (and one negative).
- Don't say it works for all languages — TypeScript/TSX only.
- Don't say the skeletons are summaries written by an AI — they are
  deterministic AST transformations; nothing is paraphrased or invented.
- Don't present dead-code findings as safe to auto-delete.

## Glossary (define on first use in the video)

- **MCP (Model Context Protocol):** open standard connecting AI assistants
  to tools/data over JSON-RPC; the assistant discovers "resources" (readable
  data) and "tools" (callable functions).
- **AST (Abstract Syntax Tree):** the parsed structure of source code that
  makes precise, deterministic code surgery possible.
- **Skeleton:** a file's structure with implementation bodies removed —
  signatures, types, classes, imports, JSDoc kept.
- **Fan-in / fan-out:** how many files import a file / how many it imports;
  high both = a refactor bottleneck.
- **Token:** the unit LLMs read and bill by; roughly ¾ of a word or ~4 bytes
  of code.
