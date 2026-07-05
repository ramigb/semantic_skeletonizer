use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::AppState;
use crate::skeleton;

pub const GLOBAL_URI: &str = "skeleton://project/global";
const FILE_URI_PREFIX: &str = "skeleton://project/file/";

pub fn file_uri(key: &str) -> String {
    format!("{}{}", FILE_URI_PREFIX, key)
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

/// Dispatch a single MCP request. Returns `None` when no response should be
/// written (JSON-RPC notifications carry no id).
pub async fn handle_request(state: &Arc<AppState>, req: Request) -> Option<Response> {
    let mut response_value = None;
    let mut error_value = None;

    match req.method.as_str() {
        "initialize" => {
            response_value = Some(json!({
                "protocolVersion": "2024-11-05",
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
                    "version": "0.1.0"
                }
            }));
        }
        "resources/list" => {
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

            response_value = Some(json!({ "resources": resources }));
        }
        "resources/read" => {
            if let Some(params) = req.params {
                if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                    if uri == GLOBAL_URI {
                        if state.skeleton_graph.is_empty() {
                            error_value = Some(json!({
                                "code": -32603,
                                "message": "Graph is empty. No files scanned or found."
                            }));
                        } else {
                            let mut graph = HashMap::new();
                            for entry in state.skeleton_graph.iter() {
                                graph.insert(entry.key().clone(), entry.value().clone());
                            }
                            response_value = Some(json!({
                                "contents": [{
                                    "uri": GLOBAL_URI,
                                    "mimeType": "application/json",
                                    "text": serde_json::to_string(&graph).unwrap_or_default()
                                }]
                            }));
                        }
                    } else if uri.starts_with(FILE_URI_PREFIX) {
                        let path = uri.trim_start_matches(FILE_URI_PREFIX);
                        let key = state.key_for(path).unwrap_or_else(|| path.to_string());
                        if let Some(file_skeleton) = state.skeleton_graph.get(&key) {
                            response_value = Some(json!({
                                "contents": [{
                                    "uri": uri,
                                    "mimeType": "application/json",
                                    "text": serde_json::to_string(&*file_skeleton).unwrap_or_default()
                                }]
                            }));
                        } else {
                            error_value = Some(json!({
                                "code": -32602,
                                "message": "File not found in graph."
                            }));
                        }
                    } else {
                        error_value = Some(json!({
                            "code": -32602,
                            "message": "Invalid URI scheme for resource."
                        }));
                    }
                }
            }
        }
        "tools/list" => {
            response_value = Some(json!({
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
            }));
        }
        "tools/call" => {
            if let Some(params) = req.params {
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments");

                match name {
                    "get_implementation" => {
                        if let Some(args) = args {
                            let p = args.get("file_path").and_then(|s| s.as_str()).unwrap_or("");
                            let n = args
                                .get("target_node")
                                .and_then(|s| s.as_str())
                                .unwrap_or("");
                            let abs = state
                                .key_for(p)
                                .map(|k| state.abs_path(&k))
                                .unwrap_or_else(|| std::path::PathBuf::from(p));
                            match skeleton::get_implementation(&abs, n) {
                                Ok(skeleton::ImplLookup::Found(source)) => {
                                    response_value = Some(json!({
                                        "content": [{ "type": "text", "text": source }]
                                    }));
                                }
                                Ok(skeleton::ImplLookup::NotFound(candidates)) => {
                                    response_value = Some(json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!(
                                                "Node '{}' not found in {}. Available top-level symbols: {}",
                                                n, p, candidates.join(", ")
                                            )
                                        }],
                                        "isError": true
                                    }));
                                }
                                Err(e) => {
                                    response_value = Some(json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!("Failed to extract implementation: {}", e)
                                        }],
                                        "isError": true
                                    }));
                                }
                            }
                        }
                    }
                    "list_symbols" | "list_functions" => {
                        if let Some(args) = args {
                            let p = args.get("file_path").and_then(|s| s.as_str()).unwrap_or("");
                            let key = state.key_for(p).unwrap_or_else(|| p.to_string());
                            if let Some(file_skeleton) = state.skeleton_graph.get(&key) {
                                let symbols: Vec<_> = file_skeleton
                                    .symbols
                                    .iter()
                                    .filter(|s| {
                                        name != "list_functions"
                                            || skeleton::CALLABLE_KINDS.contains(&s.kind.as_str())
                                    })
                                    .collect();
                                response_value = Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": serde_json::to_string(&symbols).unwrap_or_default()
                                    }]
                                }));
                            } else {
                                response_value = Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!("File not found in graph: {}", p)
                                    }],
                                    "isError": true
                                }));
                            }
                        }
                    }
                    "search_symbols" => {
                        if let Some(args) = args {
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
                            response_value = Some(json!({
                                "content": [{
                                    "type": "text",
                                    "text": serde_json::to_string(&hits).unwrap_or_default()
                                }]
                            }));
                        }
                    }
                    _ => {
                        error_value =
                            Some(json!({"code": -32601, "message": "Method not found"}));
                    }
                }
            }
        }
        _ => {
            error_value = Some(json!({"code": -32601, "message": "Method not found"}));
        }
    }

    req.id.map(|id| Response {
        jsonrpc: "2.0".to_string(),
        id: Some(id),
        result: response_value,
        error: error_value,
    })
}
