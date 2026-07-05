mod dashboard;
mod graph;
mod protocol;
mod resolve;
mod skeleton;
mod watcher;

use anyhow::Result;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use graph::{perform_initial_sweep, AppState};
use protocol::{Notification, Request};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

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
        if let Err(e) = watcher::watch_filesystem(watcher_state, notify_tx).await {
            tracing::error!("Watcher error: {}", e);
        }
    });

    let web_state = state.clone();
    tokio::spawn(async move {
        let app = dashboard::router(web_state);

        if let Ok(listener) = tokio::net::TcpListener::bind("0.0.0.0:0").await {
            if let Ok(local_addr) = listener.local_addr() {
                eprintln!(
                    "[semantic-skeletonizer] Dashboard running at http://{}",
                    local_addr
                );
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
                        let err_res = protocol::stopped_response(req.id);
                        let out = format!("{}\n", serde_json::to_string(&err_res)?);
                        state.add_log("OUT", json!(err_res));
                        stdout.write_all(out.as_bytes()).await?;
                        stdout.flush().await?;
                        line.clear();
                        continue;
                    }

                    state.add_log("IN", json!(req));
                    if let Some(res) = protocol::handle_request(&state, req).await {
                        let out = format!("{}\n", serde_json::to_string(&res)?);
                        state.add_log("OUT", json!(res));
                        stdout.write_all(out.as_bytes()).await?;
                        stdout.flush().await?;
                    }
                } else if !line.trim().is_empty() {
                    let res = protocol::parse_error_response();
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
