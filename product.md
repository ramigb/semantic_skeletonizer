# SYSTEM DIRECTIVE: BUILD EVENT-DRIVEN MCP SERVER FOR SEMANTIC SKELETONIZATION

## ROLE AND POSTURE
Act as an Expert Rust Systems Engineer. You are building a stateful, event-driven Model Context Protocol (MCP) server operating over `stdio`. The objective is to replace lossy text compression with an in-memory Topological Graph (Code Property Graph) of a TypeScript/React codebase, granting an LLM native, bidirectional sensory access to the repository.

## TECHNOLOGY STACK
* **Language:** Rust
* **Parser:** `swc` (for AST mapping and high-speed semantic extraction)
* **Protocol:** `rust-mcp-sdk` (or standard JSON-RPC over `stdio` if a minimal implementation is preferred)
* **Concurrency & IO:** `tokio` (async runtime), `notify` (filesystem watcher)

## CORE ARCHITECTURE
1. **The In-Memory Reasoning Graph:** Maintain a live global mapping of the target directory's Semantic Skeleton. Do not output text files. The graph resides in memory and updates dynamically.
2. **The Event-Driven Ingestion Engine:** Utilize `notify` to attach a filesystem watcher. Upon any `Modify` event (file save) on `.ts` or `.tsx` files, spawn a `tokio` task to isolate the changed file, recalculate its AST via `swc`, and surgically mutate its node in the in-memory graph.
3. **Semantic Skeletonization (The `swc` Visitor):** Map the AST and apply deterministic node mutations:
    * **Preserve:** External interfaces, parameter definitions, return types, class structures, and JSDoc block comments.
    * **Strip:** Internal functional logic, implementation bodies, inline comments, and pure cosmetic nodes.
    * **Result:** A high-signal, low-noise topology of the file.

## MCP PRIMITIVE IMPLEMENTATION
Implement the following exact capabilities and JSON-RPC structures:

### 1. Capabilities Declaration
The server must initialize with `resources.subscribe = true`, `resources.listChanged = true`, and `tools.listChanged = false`.

### 2. Resource Exfiltration (`resources/list` & `resources/read`)
Expose the entire in-memory skeleton as a dynamic resource.
* **URI:** `skeleton://project/global`
* **MimeType:** `application/json`
* **Behavior:** When the LLM calls `resources/read` for this URI, return the serialized global topology.

### 3. Delta-Triggered Notifications
When `notify` detects a file change and the AST is updated, the server must push a fire-and-forget notification to the client over `stdio` to force a context refresh.
* **Method:** `notifications/resources/updated`
* **Params:** `{"uri": "skeleton://project/global"}`

### 4. Deterministic Extraction Tools (`tools/list` & `tools/call`)
Implement two primary tools using the `#[mcp_tool]` macro or manual JSON-RPC routing:

**Tool A: `get_implementation`**
* **Description:** Extracts the complete, uncompressed internal logic of a specific function or component when deep execution context is required.
* **Inputs:** `file_path` (string), `target_node` (string).
* **Behavior:** Reloads the target file, parses it, and returns the raw, un-stripped AST body of the requested node.

**Tool B: `get_call_graph`**
* **Description:** Returns the exact inbound and outbound dependency edges for a node.
* **Inputs:** `file_path` (string), `node_name` (string).
* **Behavior:** Queries the in-memory graph to return an array of file paths that import the target, and an array of external dependencies the target relies on.

## EXECUTION PROTOCOL
Generate the complete `Cargo.toml` dependencies and the full `src/main.rs` implementation. The code must be robust, gracefully handling malformed files during live edits without crashing the background daemon. Provide no conversational filler. Output only the necessary architectural code.

## CORE AST MUTATION MODULE: `swc` SEMANTIC SKELETONIZER

Implement a custom `swc_ecma_visit::VisitMut` struct named `Skeletonizer`. This visitor must traverse the AST and execute deterministic, memory-safe mutations to strip computational logic while preserving structural topology and type definitions.

### Visitor Implementation Specifications

```rust
use swc_ecma_ast::*;
use swc_ecma_visit::{VisitMut, VisitMutWith};

pub struct Skeletonizer;

impl VisitMut for Skeletonizer {
    // 1. STANDARD FUNCTIONS: Preserve signature, erase execution logic.
    fn visit_mut_function(&mut self, n: &mut Function) {
        n.visit_mut_children_with(self);
        if let Some(body) = &mut n.body {
            body.stmts.clear(); // Leaves an empty block {}
        }
    }

    // 2. ARROW FUNCTIONS: Force into empty block statements.
    fn visit_mut_arrow_expr(&mut self, n: &mut ArrowExpr) {
        n.visit_mut_children_with(self);
        n.body = Box::new(BlockStmtOrExpr::BlockStmt(BlockStmt {
            span: swc_common::DUMMY_SP,
            stmts: vec![],
        }));
    }

    // 3. CLASS METHODS: Retain accessibility and modifiers, drop logic.
    fn visit_mut_class_method(&mut self, n: &mut ClassMethod) {
        n.visit_mut_children_with(self);
        if let Some(body) = &mut n.function.body {
            body.stmts.clear();
        }
    }
    
    // 4. DEPENDENCY PRUNING (we will do this later):
    // Instruct the agent to implement `visit_mut_module_items` to filter out 
    // strictly visual CSS-in-JS imports or test-specific boilerplate.
}