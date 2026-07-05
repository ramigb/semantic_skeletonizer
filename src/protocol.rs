use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::AppState;
use crate::skeleton;

pub const GLOBAL_URI: &str = "skeleton://project/global";
const FILE_URI_PREFIX: &str = "skeleton://project/file/";

/// Protocol versions this server implements, newest first.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Everything a URI path segment must escape, but `/` stays readable.
const URI_ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'{')
    .add(b'}');

pub fn file_uri(key: &str) -> String {
    format!("{}{}", FILE_URI_PREFIX, utf8_percent_encode(key, URI_ENCODE))
}

fn decode_file_uri(uri: &str) -> Option<String> {
    uri.strip_prefix(FILE_URI_PREFIX)
        .map(|p| percent_decode_str(p).decode_utf8_lossy().into_owned())
}

// --- MCP PROTOCOL STRUCTURES ---

#[derive(Serialize, Deserialize, Debug)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

pub fn parse_error_response() -> Response {
    Response {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: Some(json!({"code": -32700, "message": "Parse error"})),
    }
}

pub fn stopped_response(id: Option<Value>) -> Response {
    Response {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(json!({
            "code": -32000,
            "message": "MCP Server is currently stopped via dashboard."
        })),
    }
}

/// Successful tool result with a single text content item.
fn tool_text(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

/// Tool-level failure per MCP spec: a *result* flagged isError, so the model
/// can see and react to it. JSON-RPC errors stay reserved for protocol
/// violations.
fn tool_error(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn handle_initialize(params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        SUPPORTED_PROTOCOL_VERSIONS[0]
    };
    json!({
        "protocolVersion": version,
        "capabilities": {
            "resources": {
                "subscribe": true,
                "listChanged": true
            },
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "semantic-skeletonizer",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn handle_resources_list(state: &AppState) -> Value {
    let mut resources = vec![json!({
        "uri": GLOBAL_URI,
        "name": "Global Semantic Skeleton",
        "mimeType": "application/json"
    })];
    for entry in state.skeleton_graph.iter() {
        let path = entry.key();
        resources.push(json!({
            "uri": file_uri(path),
            "name": format!("Semantic Skeleton for {}", path),
            "mimeType": "application/json"
        }));
    }
    json!({ "resources": resources })
}

fn handle_resources_read(state: &AppState, params: Option<&Value>) -> Result<Value, Value> {
    let uri = params
        .and_then(|p| p.get("uri"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| json!({"code": -32602, "message": "Missing required parameter: uri"}))?;

    if uri == GLOBAL_URI {
        let mut graph = HashMap::new();
        for entry in state.skeleton_graph.iter() {
            graph.insert(entry.key().clone(), entry.value().clone());
        }
        let mut contents = vec![json!({
            "uri": GLOBAL_URI,
            "mimeType": "application/json",
            "text": serde_json::to_string(&graph).unwrap_or_else(|_| "{}".to_string())
        })];
        if graph.is_empty() {
            contents.push(json!({
                "uri": GLOBAL_URI,
                "mimeType": "text/plain",
                "text": "Note: the graph is empty — no .ts/.tsx files were found under the root."
            }));
        }
        return Ok(json!({ "contents": contents }));
    }

    if let Some(path) = decode_file_uri(uri) {
        let key = state.key_for(&path).unwrap_or(path);
        return match state.skeleton_graph.get(&key) {
            Some(file_skeleton) => Ok(json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&*file_skeleton).unwrap_or_default()
                }]
            })),
            None => Err(json!({
                "code": -32002,
                "message": format!("Resource not found: {}", uri)
            })),
        };
    }

    Err(json!({
        "code": -32002,
        "message": format!("Unknown resource URI scheme: {}", uri)
    }))
}

fn handle_subscription(state: &AppState, params: Option<&Value>, subscribe: bool) -> Result<Value, Value> {
    let uri = params
        .and_then(|p| p.get("uri"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| json!({"code": -32602, "message": "Missing required parameter: uri"}))?;
    let mut subs = state.subscriptions.write().unwrap();
    if subscribe {
        subs.insert(uri.to_string());
    } else {
        subs.remove(uri);
    }
    Ok(json!({}))
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "get_implementation",
                "description": "Returns the original source text of a single named node: a top-level function, class, class method (ClassName.methodName), arrow/function-expression binding, or the default export (target_node: \"default\").",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "target_node": { "type": "string" }
                    },
                    "required": ["file_path", "target_node"]
                }
            },
            {
                "name": "list_symbols",
                "description": "Lists every top-level symbol in a file as {name, kind, exported, signature}. Kinds: function, arrow_function, class, method, interface, type, enum, variable, component.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            },
            {
                "name": "list_functions",
                "description": "Lists callable symbols (functions, arrow functions, methods, components) in a specific file. Alias of list_symbols filtered to callable kinds.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            },
            {
                "name": "get_dependencies",
                "description": "Resolved import edges for a file: which files it imports, which files import it, and its external packages.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "direction": { "type": "string", "enum": ["in", "out", "both"], "description": "Edge direction to include (default both)." }
                    },
                    "required": ["file_path"]
                }
            },
            {
                "name": "search_symbols",
                "description": "Case-insensitive substring search for symbol names across the whole graph. Returns [{file, name, kind}].",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }
            }
        ]
    })
}

async fn handle_tools_call(state: &Arc<AppState>, params: Option<&Value>) -> Result<Value, Value> {
    let params =
        params.ok_or_else(|| json!({"code": -32602, "message": "Missing params"}))?;
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let file_path = args
        .get("file_path")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    match name {
        "get_implementation" => {
            let target = args
                .get("target_node")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let abs = state
                .key_for(&file_path)
                .map(|k| state.abs_path(&k))
                .unwrap_or_else(|| std::path::PathBuf::from(&file_path));
            // Parse work runs off the reactor.
            let lookup = tokio::task::spawn_blocking({
                let target = target.clone();
                move || skeleton::get_implementation(&abs, &target)
            })
            .await
            .map_err(|e| json!({"code": -32603, "message": format!("task failed: {}", e)}))?;

            Ok(match lookup {
                Ok(skeleton::ImplLookup::Found(source)) => tool_text(source),
                Ok(skeleton::ImplLookup::NotFound(candidates)) => tool_error(format!(
                    "Node '{}' not found in {}. Available top-level symbols: {}",
                    target,
                    file_path,
                    candidates.join(", ")
                )),
                Err(e) => tool_error(format!("Failed to extract implementation: {}", e)),
            })
        }
        "list_symbols" | "list_functions" => {
            let key = state
                .key_for(&file_path)
                .unwrap_or_else(|| file_path.clone());
            Ok(match state.skeleton_graph.get(&key) {
                Some(file_skeleton) => {
                    let symbols: Vec<_> = file_skeleton
                        .symbols
                        .iter()
                        .filter(|s| {
                            name != "list_functions"
                                || skeleton::CALLABLE_KINDS.contains(&s.kind.as_str())
                        })
                        .collect();
                    tool_text(serde_json::to_string(&symbols).unwrap_or_default())
                }
                None => tool_error(format!("File not found in graph: {}", file_path)),
            })
        }
        "get_dependencies" => {
            let direction = args
                .get("direction")
                .and_then(|s| s.as_str())
                .unwrap_or("both");
            let key = state
                .key_for(&file_path)
                .unwrap_or_else(|| file_path.clone());
            Ok(match state.skeleton_graph.get(&key) {
                Some(node) => {
                    let mut out = serde_json::Map::new();
                    if direction == "out" || direction == "both" {
                        out.insert("imports".into(), json!(node.dependencies));
                        out.insert("external".into(), json!(node.external_deps));
                    }
                    if direction == "in" || direction == "both" {
                        out.insert("imported_by".into(), json!(state.dependents_of(&key)));
                    }
                    tool_text(serde_json::to_string(&out).unwrap_or_default())
                }
                None => tool_error(format!("File not found in graph: {}", file_path)),
            })
        }
        "search_symbols" => {
            let query = args
                .get("query")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_lowercase();
            let mut hits = Vec::new();
            for entry in state.skeleton_graph.iter() {
                for sym in &entry.value().symbols {
                    if sym.name.to_lowercase().contains(&query) {
                        hits.push(json!({
                            "file": entry.key(),
                            "name": sym.name,
                            "kind": sym.kind
                        }));
                    }
                }
            }
            Ok(tool_text(serde_json::to_string(&hits).unwrap_or_default()))
        }
        other => Err(json!({
            "code": -32602,
            "message": format!("Unknown tool: {}", other)
        })),
    }
}

/// Dispatch a single MCP request. Returns `None` when no response should be
/// written (client-to-server notifications carry no id).
pub async fn handle_request(state: &Arc<AppState>, req: Request) -> Option<Response> {
    // Client notifications (initialized, cancelled, unknown ...) are ignored.
    if req.method.starts_with("notifications/") {
        return None;
    }

    let outcome: Result<Value, Value> = match req.method.as_str() {
        "initialize" => Ok(handle_initialize(req.params.as_ref())),
        "ping" => Ok(json!({})),
        "resources/list" => Ok(handle_resources_list(state)),
        "resources/read" => handle_resources_read(state, req.params.as_ref()),
        "resources/subscribe" => handle_subscription(state, req.params.as_ref(), true),
        "resources/unsubscribe" => handle_subscription(state, req.params.as_ref(), false),
        "tools/list" => Ok(tools_list()),
        "tools/call" => handle_tools_call(state, req.params.as_ref()).await,
        _ => Err(json!({"code": -32601, "message": "Method not found"})),
    };

    let (result, error) = match outcome {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };

    req.id.map(|id| Response {
        jsonrpc: "2.0".to_string(),
        id: Some(id),
        result,
        error,
    })
}
