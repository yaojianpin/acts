//! The engine's own answer about itself (`sys:status`): the host process's
//! memory and cpu usage, the store's footprint, the process-row counts, the
//! background loops' health and the effective configuration — the data an
//! operator needs to see a running engine without reaching for the
//! process's own tools.
//!
//! The wire shape is mirrored by the client (`acts-channel`'s model), so the
//! field names here and there are one contract: change both, never one.

use crate::{
    Engine, Result,
    query::{Expr, Filter, Query},
    scheduler::{HealthState, SchedulerHealth},
};
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessesToUpdate, System};

/// The `sys:status` answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// The engine crate's version — which build answered.
    pub version: String,
    /// Memory usage: the host process and the machine it runs on.
    pub memory: MemoryStatus,
    /// Cpu usage: the host process and the machine, sampled over
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`].
    pub cpu: CpuStatus,
    /// The store backend's identity and footprint, when it can measure itself.
    pub storage: StorageStatus,
    /// Process-row counts: everything the store holds and the part still
    /// running (the store keeps only un-swept rows, so "total" is the runs
    /// not yet reclaimed by the terminal-run cleanup).
    pub processes: ProcessCounts,
    /// The store-facing background loops' failure state.
    pub health: HealthStatus,
    /// The effective configuration, exactly as it was parsed from the
    /// config file (every section, including ones the status reader did not
    /// ask about). An embedder that built its config in code answers an
    /// empty object.
    pub config: serde_json::Value,
}

/// Memory usage in bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatus {
    /// The host process's resident set.
    pub rss_bytes: u64,
    /// Installed physical memory of the machine.
    pub system_total_bytes: u64,
    /// Physical memory currently available to processes.
    pub system_available_bytes: u64,
}

/// Cpu usage, in percent (0–100 per the caller's arithmetic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuStatus {
    /// The host process's share since the previous sample.
    pub process_percent: f32,
    /// The machine's global non-idle share since the previous sample.
    pub system_percent: f32,
    /// Logical cores the machine reports.
    pub logical_cores: usize,
}

/// The store backend's footprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatus {
    /// The backend's own word for itself (`memory`, `sled`, `sqlite`,
    /// `postgres`, or a custom store's).
    pub kind: String,
    /// Data size in bytes, best effort; `null` when the backend cannot
    /// measure itself.
    pub used_bytes: Option<u64>,
}

/// Process-row counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCounts {
    /// Every process row the store holds.
    pub total: usize,
    /// The rows still in flight: state `running` or `none` (a run whose root
    /// step has not been created yet). Everything else — `completed`,
    /// `error`, `aborted`, … — is settled.
    pub active: usize,
}

/// The background loops' health, overall and per loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// `healthy` when every loop is, `degraded` when any loop is skipping
    /// ticks because its store keeps failing.
    pub state: String,
    /// One entry per loop, in [`crate::scheduler::core::health`]'s order:
    /// trigger, retry, cleanup, ready, recovery, outbox, projection.
    pub loops: Vec<LoopStatus>,
}

/// One loop's failure state.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// The process rows that count as still running: a root not yet created and
/// a root in flight. The words are the proc row's state vocabulary (see
/// `TaskState::proc_state`).
const ACTIVE_STATES: [&str; 2] = ["none", "running"];

/// Collect the status answer for `engine`.
pub async fn collect(engine: &Engine) -> Result<SystemStatus> {
    let runtime = engine.runtime();

    // Both counts come from the read model's proc collection, one row
    // fetched each: `count` is the whole answer, the page is discarded.
    let store = runtime.store();
    let total = store.procs().query(&Query::new().limit(1)).await?.count;
    let active = store
        .procs()
        .query(
            &Query::new()
                .limit(1)
                .filter(Filter::and().expr(Expr::r#in("state", ACTIVE_STATES.to_vec()))),
        )
        .await?
        .count;

    let usage = store.kv().usage().await?;
    let storage = match usage {
        Some(usage) => StorageStatus {
            kind: usage.kind,
            used_bytes: usage.used_bytes,
        },
        None => StorageStatus {
            kind: "unknown".to_string(),
            used_bytes: None,
        },
    };

    let (memory, cpu) = sample_process_metrics().await;
    let health = health_status(runtime.scheduler_health());

    Ok(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        memory,
        cpu,
        storage,
        processes: ProcessCounts { total, active },
        health,
        config: serde_json::to_value(runtime.config().table.clone())
            .unwrap_or(serde_json::Value::Null),
    })
}

/// One pass of the host's memory and cpu numbers.
///
/// sysinfo derives usage rates from the difference of two samples, so the
/// pass refreshes, waits the minimum cpu interval, and refreshes again. The
/// wait is the one cost of an honest number; it runs on the async runtime as
/// a timer, not a block.
async fn sample_process_metrics() -> (MemoryStatus, CpuStatus) {
    // Our own pid, and there is nowhere to report a failure to: a host that
    // cannot even name itself answers zeroes instead of failing a status
    // read that answered everything else.
    let Some(pid) = sysinfo::get_current_pid().ok() else {
        return (
            MemoryStatus {
                rss_bytes: 0,
                system_total_bytes: 0,
                system_available_bytes: 0,
            },
            CpuStatus {
                process_percent: 0.0,
                system_percent: 0.0,
                logical_cores: 0,
            },
        );
    };
    let mut system = System::new();
    let sample = |system: &mut System| {
        system.refresh_memory();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory(),
        );
        system.refresh_cpu_usage();
    };

    sample(&mut system);
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sample(&mut system);

    let process = system.process(pid);
    let memory = MemoryStatus {
        rss_bytes: process.map(|p| p.memory()).unwrap_or(0),
        system_total_bytes: system.total_memory(),
        system_available_bytes: system.available_memory(),
    };
    let cpu = CpuStatus {
        process_percent: process.map(|p| p.cpu_usage()).unwrap_or(0.0),
        system_percent: system.global_cpu_usage(),
        logical_cores: system.cpus().len(),
    };
    (memory, cpu)
}

/// Flatten the health bundle into the wire answer: an overall word plus one
/// entry per loop.
fn health_status(health: SchedulerHealth) -> HealthStatus {
    let loops = [
        ("trigger", health.trigger),
        ("retry", health.retry),
        ("cleanup", health.cleanup),
        ("ready", health.ready),
        ("recovery", health.recovery),
        ("outbox", health.outbox),
        ("projection", health.projection),
    ]
    .into_iter()
    .map(|(name, loop_health)| LoopStatus {
        name: name.to_string(),
        state: loop_health.state.as_str().to_string(),
        consecutive_failures: loop_health.consecutive_failures,
        backoff_ticks: loop_health.backoff_ticks,
    })
    .collect::<Vec<_>>();

    let degraded = HealthState::Degraded.as_str();
    let state = if loops
        .iter()
        .any(|loop_status| loop_status.state == degraded)
    {
        degraded
    } else {
        HealthState::Healthy.as_str()
    };

    HealthStatus {
        state: state.to_string(),
        loops,
    }
}
