//! Failure state, cadence and gauges of the store-facing background loops.
//!
//! The retry and schedule-trigger timers poll the store once per tick, and a
//! single failing tick is not their problem: it is logged and the loop goes on.
//! What a *persistently* failing store must not do is keep the loop at its full
//! tick rate — every attempt is a query that cannot succeed, the log fills with
//! one error line per tick, and nothing anywhere says the loop is not doing its
//! job, so a scheduler that has been failing for hours looks exactly like one
//! with nothing to do.
//!
//! [`LoopGuard`] is the state machine that closes both gaps. It counts
//! consecutive failed ticks; from [`DEGRADE_AFTER`] on the loop is *degraded*
//! and the guard hands out a growing number of skipped ticks (doubling up to
//! [`MAX_BACKOFF_TICKS`]) before the next attempt, so a store that is down is
//! polled once per backoff window instead of once per tick. The window is
//! cleared by the first clean tick, which is therefore also the moment the
//! loop is reported healthy again: a store that comes back is picked up
//! within one window, with no separate recovery path and no restart.
//!
//! Degrading, each widening of the window and the recovery are logged with
//! the loop's name and the current streak, and the same numbers are readable
//! as [`LoopHealth`] through the runtime's `scheduler_health` read — so the
//! state can be watched programmatically instead of parsed out of error
//! lines. Everything here is pure state: atomics, gauges and cadence config;
//! the loops that drive it live in the shell.

use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};
use std::time::Duration;

use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::ActError;

/// Consecutive failed ticks after which a loop counts as degraded and starts
/// skipping ticks. Small enough that a store outage is noticed in seconds at
/// the default tick, large enough that one transient failure (a failover, a
/// dropped connection) never moves the loop out of `Healthy`.
pub(crate) const DEGRADE_AFTER: u64 = 3;

/// Widest gap between attempts, in ticks: the backoff is capped here so the
/// recovery latency stays bounded even while the outage continues — a loop
/// that lost its store for hours still probes it within `MAX_BACKOFF_TICKS`
/// ticks of the store returning.
pub(crate) const MAX_BACKOFF_TICKS: u32 = 8;

/// Whether a loop is keeping up with its ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// No failed ticks in a row beyond [`DEGRADE_AFTER`] — the loop attempts
    /// the store every tick.
    Healthy,
    /// The store has been failing for at least [`DEGRADE_AFTER`] consecutive
    /// ticks: the loop is skipping ticks between attempts.
    Degraded,
}

impl HealthState {
    /// The name used in the state's log lines, so a log field and this type
    /// cannot drift apart.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            HealthState::Healthy => "healthy",
            HealthState::Degraded => "degraded",
        }
    }
}

/// One loop's failure state, as of the moment it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopHealth {
    pub state: HealthState,
    /// Store failures since the last tick that touched the store cleanly.
    pub consecutive_failures: u64,
    /// Ticks the loop will sit out before its next attempt (`0` while healthy).
    pub backoff_ticks: u32,
}

/// Failure state of every store-facing background loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerHealth {
    /// The schedule-trigger timer (due `schedule` rows).
    pub trigger: LoopHealth,
    /// The message-retry timer (unacked deliveries and the durable-overflow
    /// replay).
    pub retry: LoopHealth,
    /// The terminal-run cleanup loop (aggregate/row/workdir reclamation).
    pub cleanup: LoopHealth,
    /// The ready/timer pass (durable ready queue and `DispatchTimeout` rounds).
    pub ready: LoopHealth,
    /// The recovery scanner (exceptional operations only).
    pub recovery: LoopHealth,
    /// The outbox retry pass (lost confirmations, deferred transports).
    pub outbox: LoopHealth,
    /// The projection repair pass (lagging rows, repair queue, boot catch-up).
    pub projection: LoopHealth,
}

/// Consecutive-failure tracker of one periodic loop.
///
/// Every field is an atomic and all *mutations* happen on the loop's own task,
/// so the loop costs one atomic op per state read and an owner can read the
/// state at any time. [`LoopGuard::attempt`] is the only method the loop must
/// call on the happy path: a healthy loop gets `true` immediately, a degraded
/// one consumes one skipped tick per call.
pub(crate) struct LoopGuard {
    /// Loop name, used as the `loop_name` field of the state's log lines.
    name: &'static str,
    failures: AtomicU64,
    skips: AtomicU32,
}

impl LoopGuard {
    pub(crate) fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            failures: AtomicU64::new(0),
            skips: AtomicU32::new(0),
        })
    }

    /// Whether this tick may touch the store. `false` means the tick is inside
    /// the backoff window of a degraded loop and the loop must skip it —
    /// without querying the store, and without logging another failure.
    pub(crate) fn attempt(&self) -> bool {
        loop {
            let skips = self.skips.load(Ordering::Acquire);
            if skips == 0 {
                return true;
            }
            if self
                .skips
                .compare_exchange_weak(skips, skips - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return false;
            }
        }
    }

    /// Record a tick that could not reach the store.
    ///
    /// Below the threshold this only counts. From it on, the streak sizes the
    /// next backoff window; the degradation itself and every widening of the
    /// window are reported, which is what keeps a long outage from logging a
    /// warning per attempt on top of the error line the loop itself emits.
    pub(crate) fn failed(&self, err: &ActError) {
        let failures = self.failures.fetch_add(1, Ordering::AcqRel) + 1;
        if failures < DEGRADE_AFTER {
            return;
        }

        let backoff = Self::backoff_ticks(failures);
        self.skips.store(backoff, Ordering::Release);

        // The attempt that degraded the loop, and the ones that widened the
        // window, are the transitions worth a log line; a further failure
        // inside an unchanged window is not (the loop's own error line
        // already recorded it).
        let widened = backoff > Self::backoff_ticks(failures - 1);
        if failures == DEGRADE_AFTER || widened {
            warn!(
                loop_name = self.name,
                state = HealthState::Degraded.as_str(),
                consecutive_failures = failures,
                backoff_ticks = backoff,
                error = %err,
                "store loop degraded"
            );
        }
    }

    /// Record a tick that reached the store cleanly: the streak and the
    /// backoff window are both cleared, so the next tick attempts again.
    pub(crate) fn recovered(&self) {
        let failures = self.failures.swap(0, Ordering::AcqRel);
        self.skips.store(0, Ordering::Release);

        if failures >= DEGRADE_AFTER {
            info!(
                loop_name = self.name,
                state = HealthState::Healthy.as_str(),
                consecutive_failures = failures,
                "store loop recovered"
            );
        } else if failures > 0 {
            debug!(
                loop_name = self.name,
                state = HealthState::Healthy.as_str(),
                consecutive_failures = failures,
                "store loop recovered"
            );
        }
    }

    /// The loop's state for an owner that reads it (see [`SchedulerHealth`]).
    pub(crate) fn snapshot(&self) -> LoopHealth {
        let consecutive_failures = self.failures.load(Ordering::Acquire);
        LoopHealth {
            state: if consecutive_failures >= DEGRADE_AFTER {
                HealthState::Degraded
            } else {
                HealthState::Healthy
            },
            consecutive_failures,
            backoff_ticks: self.skips.load(Ordering::Acquire),
        }
    }

    /// Ticks to sit out after `failures` consecutive failures: one after the
    /// first degrading failure, doubling with each further one up to
    /// [`MAX_BACKOFF_TICKS`].
    fn backoff_ticks(failures: u64) -> u32 {
        // `.min(31)` keeps the shift in `u32` range; the cap below is reached
        // long before it could matter.
        let steps = failures.saturating_sub(DEGRADE_AFTER).min(31) as u32;
        (1u32 << steps).min(MAX_BACKOFF_TICKS)
    }
}

/// What the driver's ready-queue passes observed. A gauge, not a history: the
/// last pass wins for backlog/age; the counters accumulate for the host's
/// lifetime.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadyMetrics {
    /// Ready records the last pass observed (due or not).
    pub backlog: usize,
    /// Operations the last page found due.
    pub due: usize,
    /// Age of the oldest observed ready record, in milliseconds.
    pub oldest_ready_age_ms: Option<i64>,
    /// Passes run over the host's lifetime.
    pub passes: u64,
    /// Index entries walked over the host's lifetime.
    pub scanned: u64,
    /// Operations deferred to a later poll (parked runs, idle timers).
    pub deferred: u64,
    /// Wakes that did not fit the in-memory channel.
    pub wake_failures: u64,
    /// Missing ready records repaired by the boot pass.
    pub repaired: u64,
}

/// What the outbox retry pass observed and did (A10). `backlog` and
/// `oldest_due_age_ms` are gauges of the last pass; the counters accumulate
/// for the host's lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OutboxMetrics {
    /// Due records the last pass saw in the index.
    pub backlog: usize,
    /// Age of the oldest due record, in milliseconds.
    pub oldest_due_age_ms: Option<i64>,
    /// Index entries the last pass walked (due or not).
    pub observed: usize,
    /// Retry passes run.
    pub passes: u64,
    /// Records delivered by a retry pass.
    pub delivered: u64,
    /// Records left for a later pass because their run's rows were behind.
    pub deferred: u64,
}

/// What the recovery scanner observed and did (A10). `scanned` is the backlog
/// examined by the last pass (every exceptional row is scanned by a pass);
/// the counters accumulate for the host's lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecoveryMetrics {
    /// Exceptional operations the last pass examined.
    pub scanned: usize,
    /// Operations left after the last pass's budget (the scan resumes there).
    pub more_pending: bool,
    pub passes: u64,
    /// Expired claims reclaimed into the ready queue.
    pub reclaimed: u64,
    /// Missing ready records repaired.
    pub ready_repaired: u64,
    /// Commit-prepared operations finalized.
    pub finalized: u64,
    /// Operations needing manual reconciliation, as of the last pass.
    pub reconciliation_required: usize,
    pub effect_applied: u64,
    pub effect_retried: u64,
}

/// What the terminal-run cleanup observed and reclaimed.
///
/// `backlog` and `oldest_removable_age_ms` are gauges of the last pass;
/// `reclaimed`, `document_bytes_deleted`, `delete_failures` and
/// `orphan_indexes_removed` accumulate for the runtime's lifetime.
#[derive(Clone, Copy, Debug, Default)]
pub struct CleanupMetrics {
    /// Terminal proc rows the last pass selected (capped by the cleanup
    /// batch).
    pub backlog: usize,
    /// Age in milliseconds of the oldest finished run whose row is marked
    /// `removable`; `None` when nothing is waiting.
    pub oldest_removable_age_ms: Option<i64>,
    /// Runs whose durable aggregate was reclaimed (or whose aggregate was
    /// already gone and whose marked rows were removed).
    pub reclaimed: u64,
    /// Bytes of aggregate/index documents deleted.
    pub document_bytes_deleted: u64,
    /// Cleanup attempts that failed and are retried on the next pass.
    pub delete_failures: u64,
    /// Index documents removed for an aggregate that was already gone, or
    /// stale recovery-entry documents removed with a reclaimed aggregate.
    pub orphan_indexes_removed: u64,
}

/// One cleanup pass's local counters, folded into [`CleanupMetrics`].
#[derive(Default)]
pub(crate) struct CleanupPass {
    pub(crate) backlog: usize,
    pub(crate) oldest_removable_age_ms: Option<i64>,
    pub(crate) reclaimed: u64,
    pub(crate) document_bytes: u64,
    pub(crate) delete_failures: u64,
    pub(crate) orphan_indexes: u64,
}

/// The independent cadences and per-round budgets of the kernel host (A10).
///
/// Each loop owns one interval: the ready/timer pass (normal liveness), the
/// recovery scanner (exceptional work), the outbox retry pass and the
/// projection repair pass. A slow or failing loop cannot delay another, and a
/// shorter loop never multiplies another's scans.
#[derive(Clone, Copy, Debug)]
pub(crate) struct KernelSchedule {
    pub(crate) ready: Duration,
    pub(crate) recovery: Duration,
    pub(crate) outbox: Duration,
    pub(crate) projection: Duration,
    pub(crate) ready_budget: usize,
    pub(crate) recovery_budget: usize,
    pub(crate) outbox_budget: usize,
    pub(crate) projection_budget: usize,
}

impl KernelSchedule {
    /// The pre-A10 cadence: every loop on the driver tick, with the given
    /// page budget. Used by hosts that do not configure the split.
    #[cfg(test)]
    pub(crate) fn uniform(tick: Duration, page_size: usize) -> Self {
        Self {
            ready: tick,
            recovery: tick,
            outbox: tick,
            projection: tick,
            ready_budget: page_size,
            recovery_budget: page_size,
            outbox_budget: page_size,
            projection_budget: page_size,
        }
    }
}

/// The three store-facing timers' guards and the cleanup counters, as one
/// bundle: the timer tasks hold the `Arc`, the guards' atomics take the
/// mutations, and a read of any timer's state needs no lock.
pub(crate) struct TimerGuards {
    trigger: Arc<LoopGuard>,
    retry: Arc<LoopGuard>,
    cleanup: Arc<LoopGuard>,
    cleanup_metrics: Mutex<CleanupMetrics>,
}

impl TimerGuards {
    pub(crate) fn new() -> Self {
        Self {
            trigger: LoopGuard::new("schedule-trigger"),
            retry: LoopGuard::new("message-retry"),
            cleanup: LoopGuard::new("terminal-cleanup"),
            cleanup_metrics: Mutex::new(CleanupMetrics::default()),
        }
    }

    /// The schedule-trigger timer's guard.
    pub(crate) fn trigger(&self) -> &Arc<LoopGuard> {
        &self.trigger
    }

    /// The message-retry timer's guard.
    pub(crate) fn retry(&self) -> &Arc<LoopGuard> {
        &self.retry
    }

    /// The terminal-cleanup timer's guard.
    // The cleanup loop is compiled out under `cfg(test)` — reclamation there
    // is driven explicitly by `Runtime::sweep_removable` — so this accessor
    // has no caller in the test build.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn cleanup(&self) -> &Arc<LoopGuard> {
        &self.cleanup
    }

    /// The schedule-trigger timer's failure state.
    pub(crate) fn trigger_health(&self) -> LoopHealth {
        self.trigger.snapshot()
    }

    /// The message-retry timer's failure state.
    pub(crate) fn retry_health(&self) -> LoopHealth {
        self.retry.snapshot()
    }

    /// The terminal-cleanup timer's failure state.
    pub(crate) fn cleanup_health(&self) -> LoopHealth {
        self.cleanup.snapshot()
    }

    /// Fold one cleanup pass's local counters into the gauges.
    pub(crate) fn fold_cleanup(&self, pass: &CleanupPass) {
        let mut metrics = self.cleanup_metrics.lock();
        metrics.backlog = pass.backlog;
        metrics.oldest_removable_age_ms = pass.oldest_removable_age_ms;
        metrics.reclaimed += pass.reclaimed;
        metrics.document_bytes_deleted += pass.document_bytes;
        metrics.delete_failures += pass.delete_failures;
        metrics.orphan_indexes_removed += pass.orphan_indexes;
    }

    /// What the last cleanup passes observed and reclaimed.
    pub(crate) fn cleanup_metrics(&self) -> CleanupMetrics {
        *self.cleanup_metrics.lock()
    }
}
