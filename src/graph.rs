use dashmap::DashMap;
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::skeleton::{skeletonize_file, FileSkeleton};

#[derive(Serialize, Clone)]
pub struct LogEntry {
    pub timestamp: u64,
    pub direction: String,
    pub payload: Value,
}

pub struct AppState {
    pub skeleton_graph: DashMap<String, FileSkeleton>,
    pub logs: RwLock<VecDeque<LogEntry>>,
    pub uptime_acc: RwLock<Duration>,
    pub uptime_start: RwLock<Option<Instant>>,
    pub is_running: AtomicBool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            skeleton_graph: DashMap::new(),
            logs: RwLock::new(VecDeque::new()),
            uptime_acc: RwLock::new(Duration::ZERO),
            uptime_start: RwLock::new(Some(Instant::now())),
            is_running: AtomicBool::new(true),
        }
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
