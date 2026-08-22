use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::Mutex;

use crate::metrics::disk::DiskSnapshot;
use crate::metrics::network::NetSnapshot;

#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub ticks: HashMap<u32, u64>,
    pub at: Instant,
    pub users: HashMap<u32, String>,
}

impl Default for ProcessSnapshot {
    fn default() -> Self {
        ProcessSnapshot {
            ticks: HashMap::new(),
            at: Instant::now(),
            users: HashMap::new(),
        }
    }
}

/// App-wide mutable state for metrics that require delta computation.
///
/// Held behind a `tokio::sync::Mutex` because Tauri commands are async.
pub struct MetricsState {
    pub prev_disk: Mutex<DiskSnapshot>,
    pub prev_net: Mutex<NetSnapshot>,
    pub prev_procs: Mutex<ProcessSnapshot>,
}

impl Default for MetricsState {
    fn default() -> Self {
        MetricsState {
            prev_disk: Mutex::new(DiskSnapshot::default()),
            prev_net: Mutex::new(NetSnapshot::default()),
            prev_procs: Mutex::new(ProcessSnapshot::default()),
        }
    }
}
