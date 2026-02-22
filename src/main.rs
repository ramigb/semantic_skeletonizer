use anyhow::{Context, Result};
use dashmap::DashMap;
use ignore::WalkBuilder;
use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use swc_common::{sync::Lrc, SourceMap, DUMMY_SP, SyntaxContext};
use swc_ecma_ast::*;
use swc_ecma_codegen::{text_writer::JsWriter, Config, Emitter, Node};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_ecma_visit::{VisitMut, VisitMutWith};

// --- MCP PROTOCOL STRUCTURES ---

#[derive(Serialize, Deserialize, Debug)]
struct Request {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Response {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Notification {
    jsonrpc: String,
    method: String,
    params: Value,
}

// --- IR STRUCTURES ---

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct FileSkeleton {
    imports: Vec<String>,
    exports: Vec<String>,
    functions: Vec<String>,
    interfaces: Vec<String>,
    classes: Vec<String>,
    variables: Vec<String>,
}

// --- SKELETONIZER ---

pub struct Skeletonizer;

impl VisitMut for Skeletonizer {
    fn visit_mut_function(&mut self, n: &mut Function) {
        n.visit_mut_children_with(self);
        if let Some(body) = &mut n.body {
            body.stmts.clear();
        }
    }

    fn visit_mut_arrow_expr(&mut self, n: &mut ArrowExpr) {
        n.visit_mut_children_with(self);
        n.body = Box::new(BlockStmtOrExpr::BlockStmt(BlockStmt {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            stmts: vec![],
        }));
    }

    fn visit_mut_class_method(&mut self, n: &mut ClassMethod) {
        n.visit_mut_children_with(self);
        if let Some(body) = &mut n.function.body {
            body.stmts.clear();
        }
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.retain(|item| {
            if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
                let src = format!("{:?}", import.src.value);
                if src.contains(".css") || src.contains(".scss") || src.contains(".svg") {
                    return false;
                }
            }
            true
        });
        items.visit_mut_children_with(self);
    }
}

// --- APP STATE ---

struct AppState {
    skeleton_graph: DashMap<String, FileSkeleton>,
}

impl AppState {
    fn new() -> Self {
        Self {
            skeleton_graph: DashMap::new(),
        }
    }
}

// --- CORE UTILS ---

fn parse_file(path: &Path) -> Result<(Lrc<SourceMap>, Module)> {
    let cm: Lrc<SourceMap> = Default::default();

    let fm = cm.load_file(path).context("failed to load file")?;

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: path.extension().map_or(false, |ext| ext == "tsx"),
            decorators: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    let module = parser
        .parse_typescript_module()
        .map_err(|e| anyhow::anyhow!("failed to parse module: {:?}", e.into_kind()))?;

    Ok((cm, module))
}

fn stringify_item<N: Node>(node: &N, cm: Lrc<SourceMap>) -> String {
    let mut buf = vec![];
    {
        let mut emitter = Emitter {
            cfg: Default::default(),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
        };
        node.emit_with(&mut emitter).unwrap();
    }
    String::from_utf8(buf).unwrap_or_default()
}

fn extract_ir(module: &Module, cm: Lrc<SourceMap>) -> FileSkeleton {
    let mut ir = FileSkeleton::default();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(decl) => match decl {
                ModuleDecl::Import(n) => ir.imports.push(stringify_item(n, cm.clone())),
                ModuleDecl::ExportDecl(n) => ir.exports.push(stringify_item(n, cm.clone())),
                ModuleDecl::ExportNamed(n) => ir.exports.push(stringify_item(n, cm.clone())),
                ModuleDecl::ExportDefaultDecl(n) => ir.exports.push(stringify_item(n, cm.clone())),
                ModuleDecl::ExportDefaultExpr(n) => ir.exports.push(stringify_item(n, cm.clone())),
                ModuleDecl::ExportAll(n) => ir.exports.push(stringify_item(n, cm.clone())),
                ModuleDecl::TsImportEquals(n) => ir.imports.push(stringify_item(n, cm.clone())),
                ModuleDecl::TsExportAssignment(n) => ir.exports.push(stringify_item(n, cm.clone())),
                ModuleDecl::TsNamespaceExport(n) => ir.exports.push(stringify_item(n, cm.clone())),
            },
            ModuleItem::Stmt(stmt) => match stmt {
                Stmt::Decl(decl) => match decl {
                    Decl::Class(n) => ir.classes.push(stringify_item(n, cm.clone())),
                    Decl::Fn(n) => ir.functions.push(stringify_item(n, cm.clone())),
                    Decl::Var(n) => ir.variables.push(stringify_item(n, cm.clone())),
                    Decl::TsInterface(n) => ir.interfaces.push(stringify_item(n, cm.clone())),
                    Decl::TsTypeAlias(n) => ir.interfaces.push(stringify_item(n, cm.clone())),
                    Decl::TsEnum(n) => ir.interfaces.push(stringify_item(n, cm.clone())),
                    Decl::TsModule(n) => ir.interfaces.push(stringify_item(n, cm.clone())),
                    Decl::Using(_) => {}
                },
                _ => {}
            },
        }
    }
    ir
}

fn skeletonize_file(path: &Path) -> Result<FileSkeleton> {
    let (cm, mut module) = parse_file(path)?;

    let mut skeletonizer = Skeletonizer;
    module.visit_mut_with(&mut skeletonizer);

    Ok(extract_ir(&module, cm))
}

fn perform_initial_sweep(state: &Arc<AppState>) {
    for result in WalkBuilder::new(".").build() {
        if let Ok(entry) = result {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy();
                    if ext_str == "ts" || ext_str == "tsx" {
                        if let Ok(ir) = skeletonize_file(path) {
                            state
                                .skeleton_graph
                                .insert(path.to_string_lossy().to_string(), ir);
                        }
                    }
                }
            }
        }
    }
}

// --- BACKGROUND WATCHER ---

async fn watch_filesystem(state: Arc<AppState>, tx: mpsc::Sender<()>) -> Result<()> {
    let (watch_tx, mut watch_rx) = mpsc::channel(100);

    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(event) = res {
            let _ = watch_tx.blocking_send(event);
        }
    })?;

    watcher.watch(Path::new("."), RecursiveMode::Recursive)?;

    while let Some(event) = watch_rx.recv().await {
        if let EventKind::Modify(_) = event.kind {
            let mut changed = false;
            for path in event.paths {
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy();
                    if ext_str == "ts" || ext_str == "tsx" {
                        if let Ok(ir) = skeletonize_file(&path) {
                            state
                                .skeleton_graph
                                .insert(path.to_string_lossy().to_string(), ir);
                            changed = true;
                        }
                    }
                }
            }

            if changed {
                let _ = tx.send(()).await;
            }
        }
    }

    Ok(())
}

fn get_implementation(path: &str, _target_node: &str) -> Result<Value> {
    // Return full AST node as string maybe?
    let path_buf = PathBuf::from(path);
    let (_cm, module) = parse_file(&path_buf)?;
    Ok(serde_json::to_value(&module)?)
}

// --- MAIN LOOP ---

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState::new());
    
    // PERFORM INITIAL SWEEP BEFORE ACCEPTING REQUESTS
    perform_initial_sweep(&state);

    let (notify_tx, mut notify_rx) = mpsc::channel::<()>(100);

    let watcher_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = watch_filesystem(watcher_state, notify_tx).await {
            tracing::error!("Watcher error: {}", e);
        }
    });

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        tokio::select! {
            Some(_) = notify_rx.recv() => {
                let notif = Notification {
                    jsonrpc: "2.0".to_string(),
                    method: "notifications/resources/updated".to_string(),
                    params: json!({"uri": "skeleton://project/global"}),
                };
                let out = format!("{}\n", serde_json::to_string(&notif)?);
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
            }

            res = stdin.read_line(&mut line) => {
                let bytes_read = res?;
                if bytes_read == 0 {
                    break;
                }

                if let Ok(req) = serde_json::from_str::<Request>(&line) {
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
                            let mut resources = vec![
                                json!({
                                    "uri": "skeleton://project/global",
                                    "name": "Global Semantic Skeleton",
                                    "mimeType": "application/json"
                                })
                            ];
                            
                            for entry in state.skeleton_graph.iter() {
                                let path = entry.key();
                                resources.push(json!({
                                    "uri": format!("skeleton://project/file/{}", path),
                                    "name": format!("Semantic Skeleton for {}", path),
                                    "mimeType": "application/json"
                                }));
                            }

                            response_value = Some(json!({
                                "resources": resources
                            }));
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
                                                    "text": serde_json::to_string(&graph)?
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
                                                    "text": serde_json::to_string(&*file_skeleton)?
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
                                            let n = args.get("target_node").and_then(|s| s.as_str()).unwrap_or("");
                                            if let Ok(ast) = get_implementation(p, n) {
                                                response_value = Some(json!({
                                                    "content": [{
                                                        "type": "text",
                                                        "text": serde_json::to_string(&ast)?
                                                    }]
                                                }));
                                            } else {
                                                error_value = Some(json!({"code": -32603, "message": "Failed to extract implementation"}));
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
                                                error_value = Some(json!({"code": -32602, "message": "File not found in graph."}));
                                            }
                                        }
                                    }
                                    _ => {
                                        error_value = Some(json!({"code": -32601, "message": "Method not found"}));
                                    }
                                }
                            }
                        }
                        _ => {
                            error_value = Some(json!({"code": -32601, "message": "Method not found"}));
                        }
                    }

                    if let Some(id) = req.id {
                        let res = Response {
                            jsonrpc: "2.0".to_string(),
                            id: Some(id),
                            result: response_value,
                            error: error_value,
                        };
                        let out = format!("{}\n", serde_json::to_string(&res)?);
                        stdout.write_all(out.as_bytes()).await?;
                        stdout.flush().await?;
                    }
                } else if !line.trim().is_empty() {
                    let res = Response {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(json!({"code": -32700, "message": "Parse error"})),
                    };
                    let out = format!("{}\n", serde_json::to_string(&res)?);
                    stdout.write_all(out.as_bytes()).await?;
                    stdout.flush().await?;
                }
                line.clear();
            }
        }
    }

    Ok(())
}
