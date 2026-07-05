use axum::{
    extract::{Json as AxumJson, State as AxumState},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::graph::{perform_initial_sweep, AppState, LogEntry};

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


pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/skeletons", get(list_skeletons).delete(delete_skeleton))
        .route("/api/logs", get(list_logs))
        .route("/api/status", get(get_status))
        .route("/api/control", axum::routing::post(control_server))
        .with_state(state)
}
