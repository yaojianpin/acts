//! The runtime's store-facing timer loops, as one guider the kernel host's
//! serve loop ticks.
//!
//! Each loop is one pass (`*_tick`) in its own file: the message retry
//! ([`TimerGuider::retry_tick`]), the terminal-run cleanup
//! ([`TimerGuider::cleanup_tick`]), the schedule triggers
//! ([`TimerGuider::trigger_tick`]) and the snapshot TTL purge
//! ([`TimerGuider::purge_tick`]). The guards and the cleanup counters
//! they read and write live in [`crate::scheduler::core::health`] — the state
//! is health's; this module is only the loops and the cadence math they tick
//! on. The host's serve loop owns the only task that runs them, so one
//! background service reaches the store.

mod cleanup;
mod purge;
mod retry;
mod trigger;

// Test suites drive the trigger and cleanup capabilities directly; the loops
// themselves live in the child modules.
#[cfg(test)]
pub(crate) use cleanup::sweep_removable;
#[cfg(test)]
pub(crate) use trigger::fire_schedule;

use std::sync::Arc;
use std::time::Duration;

use crate::Config;
use crate::scheduler::Runtime;
use crate::scheduler::core::health::{KernelSchedule, TimerGuards};

#[cfg(test)]
use crate::scheduler::runtime::TEST_TICK_MS;

/// The default interval every unset loop follows: the configured tick, or
/// the engine's default of 15s, and [`TEST_TICK_MS`] under test (A10).
pub(crate) fn tick_interval(config: &Config) -> Duration {
    #[cfg(test)]
    {
        let _ = config;
        Duration::from_millis(TEST_TICK_MS)
    }
    #[cfg(not(test))]
    {
        let secs = config.tick_interval_secs();
        let secs = if secs > 0 { secs } else { 15 };
        Duration::from_millis((secs * 1000) as u64)
    }
}

/// One loop's own interval: an explicitly configured value wins; an unset
/// one follows `fallback` (the tick). Under test an unset value keeps the
/// historical [`TEST_TICK_MS`] cadence, while an explicit value is honored
/// so a test can move one loop without moving the others.
pub(crate) fn loop_interval(
    explicit: Option<i64>,
    configured_secs: i64,
    fallback: Duration,
) -> Duration {
    #[cfg(test)]
    {
        let _ = configured_secs;
        explicit.map_or(fallback, |secs| {
            Duration::from_millis((secs.max(1) * 1000) as u64)
        })
    }
    #[cfg(not(test))]
    {
        let _ = explicit;
        let _ = fallback;
        Duration::from_millis((configured_secs.max(1) * 1000) as u64)
    }
}

/// The independent cadences and budgets of the kernel host (A10).
///
/// Every interval defaults to the tick, so the default configuration keeps
/// the pre-A10 rhythm; setting one only moves that loop.
pub(crate) fn kernel_schedule(config: &Config) -> KernelSchedule {
    let tick = tick_interval(config);
    KernelSchedule {
        ready: loop_interval(
            config.data.ready_interval_secs,
            config.ready_interval_secs(),
            tick,
        ),
        recovery: loop_interval(
            config.data.recovery_interval_secs,
            config.recovery_interval_secs(),
            tick,
        ),
        outbox: loop_interval(
            config.data.outbox_interval_secs,
            config.outbox_interval_secs(),
            tick,
        ),
        projection: loop_interval(
            config.data.projection_interval_secs,
            config.projection_interval_secs(),
            tick,
        ),
        trigger: loop_interval(
            config.data.trigger_interval_secs,
            config.trigger_interval_secs(),
            tick,
        ),
        retry: loop_interval(
            config.data.retry_interval_secs,
            config.retry_interval_secs(),
            tick,
        ),
        cleanup: cleanup_interval(config),
        snapshot: loop_interval(
            config.data.snapshot_interval_secs,
            config.snapshot_interval_secs(),
            tick,
        ),
        ready_budget: config.ready_batch(),
        recovery_budget: config.recovery_batch(),
        outbox_budget: config.outbox_batch(),
        projection_budget: config.projection_batch(),
    }
}

/// The terminal-cleanup loop's own interval: the configured cleanup interval.
/// It never shares the driver's tick or the retry timer's schedule (A10 keeps
/// the loops separate; A11 gives cleanup its own).
pub(crate) fn cleanup_interval(config: &Config) -> Duration {
    #[cfg(test)]
    {
        let _ = config;
        Duration::from_millis(TEST_TICK_MS)
    }
    #[cfg(not(test))]
    {
        Duration::from_millis((config.cleanup_interval_secs() * 1000) as u64)
    }
}

/// The runtime's store-facing timers, attached to the kernel host: the host's
/// serve loop ticks each timer on its own interval (`*_tick`), so ONE
/// background task owns every pass that reaches the store. The guider holds
/// the shared [`TimerGuards`] — the failure state `scheduler_health` reads —
/// and the runtime handle the passes serve.
pub(crate) struct TimerGuider {
    runtime: Arc<Runtime>,
    timers: Arc<TimerGuards>,
}

impl TimerGuider {
    pub(crate) fn new(runtime: &Arc<Runtime>) -> Self {
        Self {
            timers: runtime.kernel().timer_guards().clone(),
            runtime: runtime.clone(),
        }
    }
}
