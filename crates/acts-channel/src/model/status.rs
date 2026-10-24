//! The wire shape of `sys:status` — the client-side mirror of the engine's
//! answer (`acts`'s `export::status`). One contract, two copies: change both,
//! never one.

use serde::{Deserialize, Serialize};

/// The `sys:status` answer.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SystemStatus {
    /// The engine's version — which build answered.
    pub version: String,
    /// Memory usage: the server process and the machine it runs on.
    pub memory: MemoryStatus,
    /// Cpu usage: the server process and the machine.
    pub cpu: CpuStatus,
    /// The store backend's identity and footprint, when it can measure itself.
    pub storage: StorageStatus,
    /// Process-row counts: everything the store holds and the part still
    /// running.
    pub processes: ProcessCounts,
    /// The background loops' health, overall and per loop.
    pub health: HealthStatus,
    /// The server's effective configuration, as parsed from its config file.
    pub config: serde_json::Value,
}

/// Memory usage in bytes.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MemoryStatus {
    /// The server process's resident set.
    pub rss_bytes: u64,
    /// Installed physical memory of the machine.
    pub system_total_bytes: u64,
    /// Physical memory currently available to processes.
    pub system_available_bytes: u64,
}

/// Cpu usage, in percent.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CpuStatus {
    /// The server process's share since the previous sample.
    pub process_percent: f32,
    /// The machine's global non-idle share since the previous sample.
    pub system_percent: f32,
    /// Logical cores the machine reports.
    pub logical_cores: usize,
}

/// The store backend's footprint.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StorageStatus {
    /// The backend's own word for itself (`memory`, `sled`, `sqlite`,
    /// `postgres`, or a custom store's).
    pub kind: String,
    /// Data size in bytes, best effort; `null` when the backend cannot
    /// measure itself.
    pub used_bytes: Option<u64>,
}

/// Process-row counts.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcessCounts {
    /// Every process row the store holds.
    pub total: usize,
    /// The rows still in flight (state `running`, or `none` — a run whose
    /// root step has not been created yet).
    pub active: usize,
}

/// The background loops' health, overall and per loop.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HealthStatus {
    /// `healthy` when every loop is, `degraded` when any loop is skipping
    /// ticks because its store keeps failing.
    pub state: String,
    /// One entry per loop: trigger, retry, cleanup, ready, recovery, outbox,
    /// projection.
    pub loops: Vec<LoopStatus>,
}

/// One loop's failure state.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoopStatus {
    /// The loop's name (`trigger`, `retry`, `cleanup`, `ready`, `recovery`,
    /// `outbox`, `projection`).
    pub name: String,
    /// `healthy` or `degraded`.
    pub state: String,
    /// Store failures in a row since the last clean tick.
    pub consecutive_failures: u64,
    /// Ticks the loop sits out before its next attempt (`0` while healthy).
    pub backoff_ticks: u32,
}

impl From<SystemStatus> for serde_json::Value {
    fn from(status: SystemStatus) -> Self {
        serde_json::to_value(status).unwrap_or(serde_json::Value::Null)
    }
}
