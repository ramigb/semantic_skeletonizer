mod dashboard;
mod graph;
mod protocol;
mod resolve;
mod skeleton;
mod watcher;

use anyhow::{Context, Result};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use graph::{perform_initial_sweep, AppState};
use protocol::{Notification, Request};
use watcher::ChangeSet;

fn parse_root_arg() -> Result<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let root = match args.iter().position(|a| a == "--root") {
        Some(i) => PathBuf::from(
            args.get(i + 1)
                .context("--root requires a directory argument")?,
        ),
        None => std::env::current_dir()?,
    };
    root.canonicalize()
        .with_context(|| format!("cannot resolve root directory {}", root.display()))
}

async fn write_notification(
    stdout: &mut tokio::io::Stdout,
    state: &Arc<AppState>,
    method: &str,
    params: serde_json::Value,
) -> Result<()> {
    let notif = Notification {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params,
    };
    let out = format!("{}\n", serde_json::to_string(&notif)?);
    state.add_log("OUT", json!(notif));
    tokio::io::AsyncWriteExt::write_all(stdout, out.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let root = parse_root_arg()?;
    let state = Arc::new(AppState::new(root));

    let (notify_tx, mut notify_rx) = mpsc::channel::<ChangeSet>(100);

    // Sweep in the background so `initialize` is answered immediately —
    // MCP clients enforce startup timeouts, and a large or slow tree can
    // take a while. A list_changed notification announces completion.
    let sweep_state = state.clone();
    let sweep_tx = notify_tx.clone();
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let walk_state = sweep_state.clone();
        let added = tokio::task::spawn_blocking(move || perform_initial_sweep(&walk_state))
            .await
            .unwrap_or_default();
        eprintln!(
            "[semantic-skeletonizer] initial sweep: {} files in {:.2?}",
            added.len(),
            started.elapsed()
        );
        sweep_state.add_log(
            "SYS",
            json!({"event": "initial_sweep_complete", "files": added.len()}),
        );
        let _ = sweep_tx
            .send(ChangeSet {
                added,
                force_list_changed: true,
                ..ChangeSet::default()
            })
            .await;
    });

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

    // `next_line()` is cancellation-safe: a pushed notification racing in the
    // select loop can no longer drop partially-read request bytes.
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    loop {
        tokio::select! {
            Some(changes) = notify_rx.recv() => {
                let subs = state.subscriptions.read().unwrap().clone();
                // Per-URI `updated` pushes go only to subscribed URIs;
                // `list_changed` is always allowed.
                for key in changes.updated.iter().chain(changes.added.iter()) {
                    let uri = protocol::file_uri(key);
                    if subs.contains(&uri) {
                        write_notification(
                            &mut stdout,
                            &state,
                            "notifications/resources/updated",
                            json!({"uri": uri}),
                        )
                        .await?;
                    }
                }
                if subs.contains(protocol::GLOBAL_URI) {
                    write_notification(
                        &mut stdout,
                        &state,
                        "notifications/resources/updated",
                        json!({"uri": protocol::GLOBAL_URI}),
                    )
                    .await?;
                }
                if changes.force_list_changed
                    || !changes.added.is_empty()
                    || !changes.removed.is_empty()
                {
                    write_notification(
                        &mut stdout,
                        &state,
                        "notifications/resources/list_changed",
                        json!({}),
                    )
                    .await?;
                }
            }

            res = lines.next_line() => {
                let Some(line) = res? else {
                    break;
                };

                if let Ok(req) = serde_json::from_str::<Request>(&line) {
                    if !state.is_running.load(Ordering::SeqCst) {
                        state.add_log("IN", json!(req));
                        let err_res = protocol::stopped_response(req.id);
                        let out = format!("{}\n", serde_json::to_string(&err_res)?);
                        state.add_log("OUT", json!(err_res));
                        stdout.write_all(out.as_bytes()).await?;
                        stdout.flush().await?;
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
            }
        }
    }

    Ok(())
}
