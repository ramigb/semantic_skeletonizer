use anyhow::Result;
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::graph::{is_skeleton_target, AppState};
use crate::skeleton::skeletonize_file;

const DEBOUNCE: Duration = Duration::from_millis(200);

/// One coalesced batch of graph mutations, sent to the main loop so it can
/// emit MCP notifications.
#[derive(Debug, Default)]
pub struct ChangeSet {
    /// Keys whose skeleton was recomputed (existing nodes).
    pub updated: Vec<String>,
    /// Keys newly added to the graph.
    pub added: Vec<String>,
    /// Keys removed from the graph.
    pub removed: Vec<String>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.updated.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

pub async fn watch_filesystem(state: Arc<AppState>, tx: mpsc::Sender<ChangeSet>) -> Result<()> {
    let (watch_tx, mut watch_rx) = mpsc::channel(1024);

    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(event) = res {
            let _ = watch_tx.blocking_send(event);
        }
    })?;

    watcher.watch(&state.root, RecursiveMode::Recursive)?;

    // Coalesce events per path over a debounce window before re-parsing.
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();

    loop {
        let next_deadline = pending.values().min().map(|t| *t + DEBOUNCE);

        tokio::select! {
            maybe_event = watch_rx.recv() => {
                let Some(event) = maybe_event else { break };
                if !state.is_running.load(Ordering::SeqCst) {
                    continue;
                }
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                for path in event.paths {
                    // Create/Modify/Remove/Rename all funnel through the same
                    // pending set; at flush time the filesystem is the source
                    // of truth (file exists -> upsert, gone -> remove).
                    if is_skeleton_target(&path) && !state.is_ignored(&path) {
                        pending.insert(path, Instant::now());
                    }
                }
            }

            _ = tokio::time::sleep_until(next_deadline.unwrap_or_else(Instant::now)),
              if next_deadline.is_some() => {
                let now = Instant::now();
                let due: Vec<PathBuf> = pending
                    .iter()
                    .filter(|(_, t)| now.duration_since(**t) >= DEBOUNCE)
                    .map(|(p, _)| p.clone())
                    .collect();
                if due.is_empty() {
                    continue;
                }
                for p in &due {
                    pending.remove(p);
                }

                let mut changes = ChangeSet::default();
                for path in due {
                    let Some(key) = state.key_for(&path.to_string_lossy()) else {
                        continue;
                    };
                    if path.is_file() {
                        // Parse off the reactor; a malformed file mid-edit must
                        // never crash the daemon or evict the last good node.
                        let parsed = tokio::task::spawn_blocking({
                            let path = path.clone();
                            move || skeletonize_file(&path)
                        })
                        .await;
                        match parsed {
                            Ok(Ok(ir)) => {
                                if state.upsert(key.clone(), ir) {
                                    changes.added.push(key);
                                } else {
                                    changes.updated.push(key);
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::warn!(
                                    "watcher: keeping previous node for {}: {}",
                                    key,
                                    e
                                );
                            }
                            Err(e) => tracing::error!("watcher: parse task panicked: {}", e),
                        }
                    } else if state.remove(&key) {
                        changes.removed.push(key);
                    }
                }

                if !changes.is_empty() {
                    let _ = tx.send(changes).await;
                }
            }
        }
    }

    Ok(())
}
