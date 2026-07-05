use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::AppState;
use crate::skeleton;

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
                "uri": "skeleton://project/global",
                "name": "Global Semantic Skeleton",
                "mimeType": "application/json"
            })];

            for entry in state.skeleton_graph.iter() {
                let path = entry.key();
                resources.push(json!({
                    "uri": format!("skeleton://project/file/{}", path),
                    "name": format!("Semantic Skeleton for {}", path),
                    "mimeType": "application/json"
                }));
            }

            response_value = Some(json!({ "resources": resources }));
        }
        "resources/read" => {
            if let Some(params) = req.params {
                if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                    if uri == "skeleton://project/global" {
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
                                    "uri": "skeleton://project/global",
                                    "mimeType": "application/json",
                                    "text": serde_json::to_string(&graph).unwrap_or_default()
                                }]
                            }));
                        }
                    } else if uri.starts_with("skeleton://project/file/") {
                        let path = uri.trim_start_matches("skeleton://project/file/");
                        if let Some(file_skeleton) = state.skeleton_graph.get(path) {
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
                        "description": "Extracts complete inner logic of a node.",
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
                        "name": "list_functions",
                        "description": "Lists all functions in a specific file.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "file_path": { "type": "string" }
                            },
                            "required": ["file_path"]
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
                            if let Ok(ast) = skeleton::get_implementation(p, n) {
                                response_value = Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": serde_json::to_string(&ast).unwrap_or_default()
                                    }]
                                }));
                            } else {
                                error_value = Some(json!({
                                    "code": -32603,
                                    "message": "Failed to extract implementation"
                                }));
                            }
                        }
                    }
                    "list_functions" => {
                        if let Some(args) = args {
                            let p = args.get("file_path").and_then(|s| s.as_str()).unwrap_or("");
                            if let Some(file_skeleton) = state.skeleton_graph.get(p) {
                                response_value = Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": file_skeleton.functions.join("\n")
                                    }]
                                }));
                            } else {
                                error_value = Some(json!({
                                    "code": -32602,
                                    "message": "File not found in graph."
                                }));
                            }
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
