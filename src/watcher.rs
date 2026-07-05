use anyhow::Result;
use notify::{EventKind, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::graph::AppState;
use crate::skeleton::skeletonize_file;

pub async fn watch_filesystem(state: Arc<AppState>, tx: mpsc::Sender<()>) -> Result<()> {
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
