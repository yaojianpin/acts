//! Failure state of the store-facing background loops.
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
//! cleared by the first clean tick, which is therefore also the moment the loop
//! is reported healthy again: a store that comes back is picked up within one
//! window, with no separate recovery path and no restart.
//!
//! Degrading, each widening of the window and the recovery are logged with the
//! loop's name and the current streak, and the same numbers are readable as
//! [`LoopHealth`] through
//! [`Runtime::scheduler_health`](super::Runtime::scheduler_health) — so the
//! state can be watched programmatically instead of parsed out of error lines.

use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

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
    /// The message-retry timer (unacked deliveries, settled-process sweep and
    /// the durable-overflow replay).
    pub retry: LoopHealth,
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
