use dashmap::DashMap;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::skeleton::{skeletonize_file, FileSkeleton};

/// Normalize `p` (absolute, or relative to `root`) into the graph's canonical
/// key form: repo-root-relative with forward slashes, `.`/`..` resolved
/// logically (no filesystem access, so keys for deleted files still resolve).
/// Returns `None` for paths outside `root`.
pub fn canonical_key(root: &Path, p: &Path) -> Option<String> {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };

    let mut prefix = PathBuf::new();
    let mut stack: Vec<&std::ffi::OsStr> = Vec::new();
    for comp in joined.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                prefix.push(comp.as_os_str());
                stack.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::Normal(s) => stack.push(s),
        }
    }

    let mut normalized = prefix;
    for s in &stack {
        normalized.push(s);
    }

    let rel = normalized.strip_prefix(root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

pub fn is_skeleton_target(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts") | Some("tsx")
    )
}

#[derive(Serialize, Clone)]
pub struct LogEntry {
    pub timestamp: u64,
    pub direction: String,
    pub payload: Value,
}

pub struct AppState {
    pub root: PathBuf,
    pub gitignore: Gitignore,
    pub skeleton_graph: DashMap<String, FileSkeleton>,
    pub logs: RwLock<VecDeque<LogEntry>>,
    pub uptime_acc: RwLock<Duration>,
    pub uptime_start: RwLock<Option<Instant>>,
    pub is_running: AtomicBool,
}

impl AppState {
    pub fn new(root: PathBuf) -> Self {
        let mut builder = GitignoreBuilder::new(&root);
        builder.add(root.join(".gitignore"));
        let gitignore = builder.build().unwrap_or_else(|_| Gitignore::empty());

        Self {
            root,
            gitignore,
            skeleton_graph: DashMap::new(),
            logs: RwLock::new(VecDeque::new()),
            uptime_acc: RwLock::new(Duration::ZERO),
            uptime_start: RwLock::new(Some(Instant::now())),
            is_running: AtomicBool::new(true),
        }
    }

    /// Normalize arbitrary tool/resource path input (`src/x.ts`, `./src/x.ts`,
    /// absolute) into a canonical graph key.
    pub fn key_for(&self, input: &str) -> Option<String> {
        canonical_key(&self.root, Path::new(input))
    }

    /// Absolute filesystem path for a canonical graph key.
    pub fn abs_path(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    /// True if the path should never enter the graph: outside the root,
    /// inside `.git/`/`node_modules/`, or matched by `.gitignore`.
    pub fn is_ignored(&self, abs: &Path) -> bool {
        let Ok(rel) = abs.strip_prefix(&self.root) else {
            return true;
        };
        if rel
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some(".git") | Some("node_modules")))
        {
            return true;
        }
        self.gitignore
            .matched_path_or_any_parents(rel, false)
            .is_ignore()
    }

    /// Insert or replace a node. Returns `true` when the key is new.
    pub fn upsert(&self, key: String, skeleton: FileSkeleton) -> bool {
        self.skeleton_graph.insert(key, skeleton).is_none()
    }

    /// Remove a node. Returns `true` when it existed.
    pub fn remove(&self, key: &str) -> bool {
        self.skeleton_graph.remove(key).is_some()
    }

    pub fn add_log(&self, direction: &str, payload: Value) {
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

pub fn perform_initial_sweep(state: &Arc<AppState>) {
    for entry in WalkBuilder::new(&state.root).build().flatten() {
        let path = entry.path();
        if path.is_file() && is_skeleton_target(path) && !state.is_ignored(path) {
            let Some(key) = canonical_key(&state.root, path) else {
                continue;
            };
            match skeletonize_file(path) {
                Ok(ir) => {
                    state.upsert(key, ir);
                }
                Err(e) => {
                    tracing::warn!("initial sweep: skipping {}: {}", path.display(), e);
                }
            }
        }
    }
}
