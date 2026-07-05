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

use std::collections::HashSet;

use crate::resolve::{FsResolver, Resolution, Resolver};
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
    pub resolver: Box<dyn Resolver>,
    pub skeleton_graph: DashMap<String, FileSkeleton>,
    /// Reverse dependency index: key -> set of files importing it.
    pub dependents: DashMap<String, HashSet<String>>,
    /// Resource URIs the client subscribed to via resources/subscribe.
    pub subscriptions: RwLock<HashSet<String>>,
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
            resolver: Box::new(FsResolver),
            skeleton_graph: DashMap::new(),
            dependents: DashMap::new(),
            subscriptions: RwLock::new(HashSet::new()),
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

    /// Insert or replace a node, resolving its import records into graph
    /// edges and maintaining the reverse-dependency index.
    /// Returns `true` when the key is new.
    pub fn upsert(&self, key: String, mut skeleton: FileSkeleton) -> bool {
        let mut deps = Vec::new();
        let mut externals = Vec::new();
        for record in &skeleton.import_records {
            match self.resolver.resolve(&self.root, &key, &record.source) {
                Resolution::Internal(k) if k != key => deps.push(k),
                Resolution::External(pkg) => externals.push(pkg),
                _ => {}
            }
        }
        deps.sort();
        deps.dedup();
        externals.sort();
        externals.dedup();
        skeleton.dependencies = deps.clone();
        skeleton.external_deps = externals;

        let old = self.skeleton_graph.insert(key.clone(), skeleton);
        if let Some(old) = &old {
            for gone in old.dependencies.iter().filter(|d| !deps.contains(d)) {
                if let Some(mut set) = self.dependents.get_mut(gone) {
                    set.remove(&key);
                }
            }
        }
        for dep in &deps {
            self.dependents
                .entry(dep.clone())
                .or_default()
                .insert(key.clone());
        }
        old.is_none()
    }

    /// Remove a node and its outgoing edges from the reverse-dependency
    /// index. Returns `true` when it existed.
    pub fn remove(&self, key: &str) -> bool {
        match self.skeleton_graph.remove(key) {
            Some((_, old)) => {
                for dep in &old.dependencies {
                    if let Some(mut set) = self.dependents.get_mut(dep) {
                        set.remove(key);
                    }
                }
                true
            }
            None => false,
        }
    }

    /// Files that import `key`, from the reverse index.
    pub fn dependents_of(&self, key: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .dependents
            .get(key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_normalizes_everything_to_root_relative() {
        let root = Path::new("/repo");
        assert_eq!(
            canonical_key(root, Path::new("src/x.ts")).as_deref(),
            Some("src/x.ts")
        );
        assert_eq!(
            canonical_key(root, Path::new("./src/x.ts")).as_deref(),
            Some("src/x.ts")
        );
        assert_eq!(
            canonical_key(root, Path::new("/repo/./src/x.ts")).as_deref(),
            Some("src/x.ts")
        );
        assert_eq!(
            canonical_key(root, Path::new("src/sub/../x.ts")).as_deref(),
            Some("src/x.ts")
        );
        assert_eq!(
            canonical_key(root, Path::new("/repo/src/x.ts")).as_deref(),
            Some("src/x.ts")
        );
    }

    #[test]
    fn canonical_key_rejects_paths_outside_root() {
        let root = Path::new("/repo");
        assert_eq!(canonical_key(root, Path::new("/elsewhere/x.ts")), None);
        assert_eq!(canonical_key(root, Path::new("../x.ts")), None);
        assert_eq!(canonical_key(root, Path::new("src/../../x.ts")), None);
        assert_eq!(canonical_key(root, Path::new(".")), None);
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
