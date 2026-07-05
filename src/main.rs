use anyhow::{Context, Result};
use dashmap::DashMap;
use ignore::WalkBuilder;
use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use axum::{
    extract::{Json as AxumJson, State as AxumState},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_codegen::{Codegen, Gen};
use oxc_ast_visit::VisitMut;
use oxc_ast::ast::*;
use oxc_syntax::scope::ScopeFlags;

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

pub struct Skeletonizer<'a> {
    pub allocator: &'a Allocator,
}

impl<'a> VisitMut<'a> for Skeletonizer<'a> {
    fn visit_function(&mut self, func: &mut Function<'a>, flags: ScopeFlags) {
        if let Some(body) = &mut func.body {
            body.statements.clear();
        }
        oxc_ast_visit::walk_mut::walk_function(self, func, flags);
    }

    fn visit_arrow_function_expression(&mut self, expr: &mut ArrowFunctionExpression<'a>) {
        expr.body.statements.clear();
        oxc_ast_visit::walk_mut::walk_arrow_function_expression(self, expr);
    }

    fn visit_program(&mut self, program: &mut Program<'a>) {
        program.body.retain(|stmt| {
            if let Statement::ImportDeclaration(import) = stmt {
                let src = import.source.value.as_str();
                if src.contains(".css") || src.contains(".scss") || src.contains(".svg") {
                    return false;
                }
            }
            true
        });
        
        for stmt in program.body.iter_mut() {
            self.visit_statement(stmt);
        }
    }
}

// --- APP STATE ---

#[derive(Serialize, Clone)]
struct LogEntry {
    timestamp: u64,
    direction: String,
    payload: Value,
}

struct AppState {
    skeleton_graph: DashMap<String, FileSkeleton>,
    logs: RwLock<VecDeque<LogEntry>>,
    uptime_acc: RwLock<Duration>,
    uptime_start: RwLock<Option<Instant>>,
    is_running: AtomicBool,
}

impl AppState {
    fn new() -> Self {
        Self {
            skeleton_graph: DashMap::new(),
            logs: RwLock::new(VecDeque::new()),
            uptime_acc: RwLock::new(Duration::ZERO),
            uptime_start: RwLock::new(Some(Instant::now())),
            is_running: AtomicBool::new(true),
        }
    }

    fn add_log(&self, direction: &str, payload: Value) {
        let mut logs = self.logs.write().unwrap();
        if logs.len() >= 200 {
            logs.pop_front();
        }
        logs.push_back(LogEntry {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            direction: direction.to_string(),
            payload,
        });
    }
}

// --- CORE UTILS ---

fn parse_file<'a>(allocator: &'a Allocator, source_text: &'a str, path: &Path) -> Result<Program<'a>> {
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let ret = Parser::new(allocator, source_text, source_type).parse();

    if ret.errors.is_empty() {
        Ok(ret.program)
    } else {
        Err(anyhow::anyhow!("failed to parse module: {:?}", ret.errors))
    }
}

fn stringify_item<'a, T: Gen>(item: &T) -> String {
    let mut codegen = Codegen::new();
    item.r#gen(&mut codegen, oxc_codegen::Context::default());
    codegen.into_source_text()
}

fn extract_ir(program: &Program<'_>) -> FileSkeleton {
    let mut ir = FileSkeleton::default();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => ir.imports.push(stringify_item(&**decl)),
            Statement::ExportNamedDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::ExportDefaultDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::ExportAllDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::TSImportEqualsDeclaration(decl) => ir.imports.push(stringify_item(&**decl)),
            Statement::TSExportAssignment(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::TSNamespaceExportDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            
            Statement::ClassDeclaration(decl) => ir.classes.push(stringify_item(&**decl)),
            Statement::FunctionDeclaration(decl) => ir.functions.push(stringify_item(&**decl)),
            Statement::VariableDeclaration(decl) => ir.variables.push(stringify_item(&**decl)),
            Statement::TSInterfaceDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            Statement::TSTypeAliasDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            Statement::TSEnumDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            Statement::TSModuleDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            
            _ => {}
        }
    }
    ir
}

fn skeletonize_file(path: &Path) -> Result<FileSkeleton> {
    let allocator = Allocator::default();
    let source_text = std::fs::read_to_string(path).context("failed to load file")?;
    let mut program = parse_file(&allocator, &source_text, path)?;

    let mut skeletonizer = Skeletonizer { allocator: &allocator };
    skeletonizer.visit_program(&mut program);

    Ok(extract_ir(&program))
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
        if !state.is_running.load(Ordering::SeqCst) {
            continue;
        }

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
    let allocator = Allocator::default();
    let path_buf = PathBuf::from(path);
    let source_text = std::fs::read_to_string(&path_buf)?;
    let program = parse_file(&allocator, &source_text, &path_buf)?;
    Ok(json!(format!("{:#?}", program)))
}

// --- WEB DASHBOARD ---

const HTML_DASHBOARD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Skeletons Dashboard</title>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;600&display=swap" rel="stylesheet">
    <style>
        :root {
            --bg-color: #0d1117;
            --text-color: #c9d1d9;
            --card-bg: rgba(22, 27, 34, 0.7);
            --border-color: #30363d;
            --hover-border: #8b949e;
            --accent: #58a6ff;
            --danger: #f85149;
            --danger-hover: #da3633;
            --success: #238636;
        }
        body {
            font-family: 'Inter', sans-serif;
            background-color: var(--bg-color);
            color: var(--text-color);
            margin: 0;
            padding: 40px;
            display: flex;
            flex-direction: column;
            align-items: center;
        }
        .header-container {
            width: 100%;
            max-width: 1200px;
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 30px;
        }
        h1 {
            color: var(--accent);
            font-weight: 600;
            margin: 0;
        }
        .status-badge {
            background-color: var(--card-bg);
            border: 1px solid var(--border-color);
            padding: 8px 16px;
            border-radius: 20px;
            font-size: 14px;
            font-weight: 600;
            display: flex;
            align-items: center;
            gap: 8px;
        }
        .status-dot {
            width: 10px;
            height: 10px;
            background-color: var(--success);
            border-radius: 50%;
            box-shadow: 0 0 8px var(--success);
        }
        .header-controls {
            display: flex;
            gap: 10px;
            margin-left: 20px;
        }
        .btn-control {
            background-color: var(--card-bg);
            color: #c9d1d9;
            border: 1px solid var(--border-color);
            border-radius: 6px;
            padding: 8px 16px;
            font-size: 13px;
            font-weight: 600;
            cursor: pointer;
            transition: all 0.2s;
        }
        .btn-control:hover {
            color: #fff;
            background-color: rgba(255, 255, 255, 0.1);
            border-color: var(--hover-border);
        }
        .btn-control:active {
            transform: scale(0.97);
        }
        .tabs {
            display: flex;
            gap: 10px;
            margin-bottom: 20px;
            width: 100%;
            max-width: 1200px;
            border-bottom: 1px solid var(--border-color);
            padding-bottom: 10px;
        }
        .tab {
            padding: 8px 16px;
            cursor: pointer;
            border-radius: 6px;
            font-weight: 600;
            color: #8b949e;
            transition: color 0.2s, background-color 0.2s;
        }
        .tab:hover {
            color: #fff;
            background-color: rgba(255, 255, 255, 0.1);
        }
        .tab.active {
            color: #fff;
            background-color: var(--border-color);
        }
        .view-section {
            display: none;
            width: 100%;
            max-width: 1200px;
        }
        .view-section.active {
            display: block;
        }
        .container {
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
            gap: 20px;
        }
        .card {
            background-color: var(--card-bg);
            border: 1px solid var(--border-color);
            border-radius: 12px;
            padding: 20px;
            box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
            backdrop-filter: blur(10px);
            transition: transform 0.2s, border-color 0.2s;
            display: flex;
            flex-direction: column;
            justify-content: space-between;
            min-height: 200px;
        }
        .card:hover {
            transform: translateY(-4px);
            border-color: var(--hover-border);
        }
        .card-header {
            font-size: 14px;
            font-weight: 600;
            word-break: break-all;
            margin-bottom: 15px;
            color: #fff;
            padding-bottom: 10px;
            border-bottom: 1px solid var(--border-color);
        }
        .stats {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 8px;
            font-size: 13px;
            color: #8b949e;
            margin-bottom: 20px;
        }
        .btn-delete {
            background-color: var(--danger);
            color: white;
            border: none;
            border-radius: 6px;
            padding: 8px 12px;
            font-size: 13px;
            font-weight: 600;
            cursor: pointer;
            transition: background-color 0.2s;
            align-self: flex-end;
            margin-top: auto;
        }
        .btn-delete:hover {
            background-color: var(--danger-hover);
        }
        .empty-state {
            text-align: center;
            color: #8b949e;
            font-size: 16px;
            grid-column: 1 / -1;
            padding: 40px;
        }
        
        /* LOGS SECTION */
        .logs-container {
            background-color: #010409;
            border: 1px solid var(--border-color);
            border-radius: 12px;
            padding: 20px;
            height: 600px;
            overflow-y: auto;
            font-family: monospace;
            font-size: 13px;
        }
        .log-entry {
            margin-bottom: 12px;
            padding-bottom: 12px;
            border-bottom: 1px dashed var(--border-color);
        }
        .log-meta {
            color: #8b949e;
            margin-bottom: 4px;
        }
        .log-in { color: #58a6ff; font-weight: bold; }
        .log-out { color: #3fb950; font-weight: bold; }
        .log-payload {
            color: #c9d1d9;
            white-space: pre-wrap;
            word-break: break-all;
        }
    </style>
</head>
<body>
    <div class="header-container">
        <h1>Skeletons Dashboard</h1>
        <div style="display: flex; align-items: center;">
            <div class="status-badge" id="status-badge">
                <div class="status-dot"></div>
                Connecting...
            </div>
            <div class="header-controls">
                <button class="btn-control" onclick="sendControl('start')">▶ Start</button>
                <button class="btn-control" onclick="sendControl('stop')">⏸ Stop</button>
                <button class="btn-control" onclick="sendControl('restart')">🔄 Restart</button>
            </div>
        </div>
    </div>
    
    <div class="tabs">
        <div class="tab active" onclick="switchTab('skeletons')">Skeletons</div>
        <div class="tab" onclick="switchTab('logs')">MCP Logs</div>
    </div>

    <div id="view-skeletons" class="view-section active">
        <div class="container" id="skeleton-list">
            <div class="empty-state">Loading...</div>
        </div>
    </div>

    <div id="view-logs" class="view-section">
        <div class="logs-container" id="logs-list">
            <div class="empty-state">Waiting for logs...</div>
        </div>
    </div>

    <script>
        let autoRefreshStatus = true;

        async function sendControl(action) {
            try {
                const response = await fetch('/api/control', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ action })
                });
                
                if (response.ok) {
                    fetchStatus();
                } else {
                    alert('Failed to send control command');
                }
            } catch (err) {
                console.error('Failed to send control command', err);
            }
        }

        function switchTab(tabId) {
            document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
            document.querySelectorAll('.view-section').forEach(v => v.classList.remove('active'));
            
            if (tabId === 'skeletons') {
                document.querySelectorAll('.tab')[0].classList.add('active');
                document.getElementById('view-skeletons').classList.add('active');
            } else {
                document.querySelectorAll('.tab')[1].classList.add('active');
                document.getElementById('view-logs').classList.add('active');
                fetchLogs();
            }
        }

        async function fetchStatus() {
            try {
                const response = await fetch('/api/status');
                const data = await response.json();
                
                const secs = data.uptime_seconds;
                const hrs = Math.floor(secs / 3600);
                const mins = Math.floor((secs % 3600) / 60);
                const rmSecs = secs % 60;
                
                let uptimeStr = '';
                if (hrs > 0) uptimeStr += `${hrs}h `;
                if (mins > 0 || hrs > 0) uptimeStr += `${mins}m `;
                uptimeStr += `${rmSecs}s`;
                
                if (data.is_running) {
                    document.getElementById('status-badge').innerHTML = `
                        <div class="status-dot"></div>
                        Online | Uptime: ${uptimeStr} | Skeletons: ${data.skeletons_count}
                    `;
                } else {
                    document.getElementById('status-badge').innerHTML = `
                        <div class="status-dot" style="background-color: #d29922; box-shadow: 0 0 8px #d29922;"></div>
                        Stopped | Uptime: ${uptimeStr} | Skeletons: ${data.skeletons_count}
                    `;
                }
                
            } catch (err) {
                document.getElementById('status-badge').innerHTML = `
                    <div class="status-dot" style="background-color: var(--danger); box-shadow: 0 0 8px var(--danger);"></div>
                    Offline
                `;
            }
        }

        async function fetchLogs() {
            try {
                const response = await fetch('/api/logs');
                const data = await response.json();
                
                const container = document.getElementById('logs-list');
                if (data.length === 0) {
                    container.innerHTML = '<div class="empty-state">No standard IO logs yet.</div>';
                    return;
                }
                
                // Format logs: newest at the bottom
                // The backend sends them chronologically if using a normal VecDeque iter
                container.innerHTML = data.map(log => {
                    const dirClass = log.direction === 'IN' ? 'log-in' : 'log-out';
                    const icon = log.direction === 'IN' ? '↓ IN' : '↑ OUT';
                    const date = new Date(log.timestamp).toLocaleTimeString();
                    
                    return `
                        <div class="log-entry">
                            <div class="log-meta">
                                <span class="${dirClass}">[${icon}]</span> 
                                <span>${date}</span>
                            </div>
                            <div class="log-payload">${JSON.stringify(log.payload, null, 2)}</div>
                        </div>
                    `;
                }).join('');
                
                // keep scrolled to bottom
                container.scrollTop = container.scrollHeight;
                
            } catch (err) {
                console.error('Failed to fetch logs', err);
            }
        }

        async function fetchSkeletons() {
            try {
                const response = await fetch('/api/skeletons');
                const data = await response.json();
                renderSkeletons(data);
            } catch (err) {
                console.error('Failed to fetch skeletons', err);
                document.getElementById('skeleton-list').innerHTML = '<div class="empty-state">Failed to load skeletons.</div>';
            }
        }

        async function deleteSkeleton(path) {
            try {
                const response = await fetch('/api/skeletons', {
                    method: 'DELETE',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ path })
                });

                if (response.ok) {
                    fetchSkeletons();
                    fetchStatus();
                } else {
                    alert('Failed to delete skeleton');
                }
            } catch (err) {
                console.error('Failed to delete skeleton', err);
                alert('Error deleting skeleton');
            }
        }

        function renderSkeletons(skeletons) {
            const container = document.getElementById('skeleton-list');
            
            if (skeletons.length === 0) {
                container.innerHTML = '<div class="empty-state">No skeletons found in memory.</div>';
                return;
            }

            container.innerHTML = skeletons.map(skel => `
                <div class="card">
                    <div>
                        <div class="card-header">${skel.path}</div>
                        <div class="stats">
                            <span>📦 Imports: ${skel.imports_count}</span>
                            <span>📤 Exports: ${skel.exports_count}</span>
                            <span>⚙️ Functions: ${skel.functions_count}</span>
                            <span>🧩 Classes: ${skel.classes_count}</span>
                            <span>📝 Interfaces: ${skel.interfaces_count}</span>
                            <span>📦 Variables: ${skel.variables_count}</span>
                        </div>
                    </div>
                    <button class="btn-delete" onclick="deleteSkeleton('${skel.path.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}')">
                        Delete
                    </button>
                </div>
            `).join('');
        }

        // Init routine
        fetchStatus();
        fetchSkeletons();
        
        // Polling loop for status and logs
        setInterval(() => {
            fetchStatus();
            if(document.getElementById('view-logs').classList.contains('active')) {
                fetchLogs();
            }
            if(document.getElementById('view-skeletons').classList.contains('active')) {
                fetchSkeletons();
            }
        }, 2000);

    </script>
</body>
</html>"#;

#[derive(Serialize)]
struct StatusResponse {
    uptime_seconds: u64,
    skeletons_count: usize,
    logs_count: usize,
    is_running: bool,
}

#[derive(Deserialize)]
struct ControlRequest {
    action: String,
}

#[derive(Serialize)]
struct SkeletonInfo {
    path: String,
    imports_count: usize,
    exports_count: usize,
    functions_count: usize,
    classes_count: usize,
    interfaces_count: usize,
    variables_count: usize,
}

#[derive(Deserialize)]
struct DeleteRequest {
    path: String,
}

async fn dashboard_handler() -> Html<&'static str> {
    Html(HTML_DASHBOARD)
}

async fn list_skeletons(AxumState(state): AxumState<Arc<AppState>>) -> impl IntoResponse {
    let mut infos = vec![];
    for entry in state.skeleton_graph.iter() {
        let skel = entry.value();
        infos.push(SkeletonInfo {
            path: entry.key().clone(),
            imports_count: skel.imports.len(),
            exports_count: skel.exports.len(),
            functions_count: skel.functions.len(),
            classes_count: skel.classes.len(),
            interfaces_count: skel.interfaces.len(),
            variables_count: skel.variables.len(),
        });
    }
    infos.sort_by(|a, b| a.path.cmp(&b.path));
    AxumJson(infos)
}

async fn delete_skeleton(
    AxumState(state): AxumState<Arc<AppState>>,
    AxumJson(req): AxumJson<DeleteRequest>,
) -> impl IntoResponse {
    if state.skeleton_graph.remove(&req.path).is_some() {
        AxumJson(json!({"success": true}))
    } else {
        AxumJson(json!({"error": "not found"}))
    }
}

async fn list_logs(AxumState(state): AxumState<Arc<AppState>>) -> impl IntoResponse {
    let logs = state.logs.read().unwrap();
    let current_logs: Vec<LogEntry> = logs.iter().cloned().collect();
    AxumJson(current_logs)
}

async fn get_status(AxumState(state): AxumState<Arc<AppState>>) -> impl IntoResponse {
    let mut uptime_seconds = state.uptime_acc.read().unwrap().as_secs();
    if let Some(start) = *state.uptime_start.read().unwrap() {
        uptime_seconds += start.elapsed().as_secs();
    }
    
    let skeletons_count = state.skeleton_graph.len();
    let logs_count = state.logs.read().unwrap().len();
    let is_running = state.is_running.load(Ordering::SeqCst);

    AxumJson(StatusResponse {
        uptime_seconds,
        skeletons_count,
        logs_count,
        is_running,
    })
}

async fn control_server(
    AxumState(state): AxumState<Arc<AppState>>,
    AxumJson(req): AxumJson<ControlRequest>,
) -> impl IntoResponse {
    match req.action.as_str() {
        "start" => {
            if !state.is_running.load(Ordering::SeqCst) {
                *state.uptime_start.write().unwrap() = Some(Instant::now());
                state.is_running.store(true, Ordering::SeqCst);
                state.add_log("SYS", json!({"event": "server_started"}));
            }
        }
        "stop" => {
            if state.is_running.load(Ordering::SeqCst) {
                if let Some(start) = *state.uptime_start.write().unwrap() {
                    *state.uptime_acc.write().unwrap() += start.elapsed();
                }
                *state.uptime_start.write().unwrap() = None;
                
                state.is_running.store(false, Ordering::SeqCst);
                state.add_log("SYS", json!({"event": "server_stopped"}));
            }
        }
        "restart" => {
            *state.uptime_acc.write().unwrap() = Duration::ZERO;
            *state.uptime_start.write().unwrap() = Some(Instant::now());
            
            state.is_running.store(false, Ordering::SeqCst);
            state.skeleton_graph.clear();
            perform_initial_sweep(&state);
            state.is_running.store(true, Ordering::SeqCst);
            state.add_log("SYS", json!({"event": "server_restarted"}));
        }
        _ => return AxumJson(json!({"error": "invalid action"})),
    }
    AxumJson(json!({"success": true}))
}

// --- MAIN LOOP ---

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let state = Arc::new(AppState::new());
    
    // PERFORM INITIAL SWEEP IN BACKGROUND
    let sweep_state = state.clone();
    tokio::spawn(async move {
        perform_initial_sweep(&sweep_state);
        sweep_state.add_log("SYS", json!({"event": "initial_sweep_complete"}));
    });

    let (notify_tx, mut notify_rx) = mpsc::channel::<()>(100);

    let watcher_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = watch_filesystem(watcher_state, notify_tx).await {
            tracing::error!("Watcher error: {}", e);
        }
    });

    let web_state = state.clone();
    tokio::spawn(async move {
        let app = Router::new()
            .route("/", get(dashboard_handler))
            .route("/api/skeletons", get(list_skeletons).delete(delete_skeleton))
            .route("/api/logs", get(list_logs))
            .route("/api/status", get(get_status))
            .route("/api/control", axum::routing::post(control_server))
            .with_state(web_state);

        if let Ok(listener) = tokio::net::TcpListener::bind("0.0.0.0:0").await {
            if let Ok(local_addr) = listener.local_addr() {
                eprintln!("[semantic-skeletonizer] Dashboard running at http://{}", local_addr);
                tracing::info!("Dashboard running at http://{}", local_addr);
            }
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("Web server error: {}", e);
            }
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
                state.add_log("OUT", json!(notif));
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
            }

            res = stdin.read_line(&mut line) => {
                let bytes_read = res?;
                if bytes_read == 0 {
                    break;
                }

                if let Ok(req) = serde_json::from_str::<Request>(&line) {
                    if !state.is_running.load(Ordering::SeqCst) {
                        state.add_log("IN", json!(req));
                        let err_res = Response {
                            jsonrpc: "2.0".to_string(),
                            id: req.id,
                            result: None,
                            error: Some(json!({
                                "code": -32000,
                                "message": "MCP Server is currently stopped via dashboard."
                            })),
                        };
                        let out = format!("{}\n", serde_json::to_string(&err_res)?);
                        state.add_log("OUT", json!(err_res));
                        stdout.write_all(out.as_bytes()).await?;
                        stdout.flush().await?;
                        line.clear();
                        continue;
                    }

                    state.add_log("IN", json!(req));
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
                        state.add_log("OUT", json!(res));
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
                    state.add_log("OUT", json!(res));
                    stdout.write_all(out.as_bytes()).await?;
                    stdout.flush().await?;
                }
                line.clear();
            }
        }
    }

    Ok(())
}
