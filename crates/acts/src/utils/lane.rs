//! The one pid → lane mapping, shared by every component that keeps one
//! process's work ordered while independent processes run concurrently: the
//! scheduler task lanes and the per-process event gate share one
//! [`LaneRouter`] instance, the store writer shards run their own. Sharing an
//! instance within a component family means the routing can never disagree —
//! and a component that must keep its work serialized against another's
//! (scheduler jobs against the event handlers of the same process) gets it
//! for free.
//!
//! Assignment is round-robin on first sight, not a hash. A hash distributes
//! in expectation only: with fewer live processes than lanes, idle lanes are
//! the norm, and a worker can starve indefinitely. Round-robin spreads the
//! assigned processes across lanes so that the per-lane counts differ by at
//! most one; a single hot process still pins its lane, which is the price of
//! the per-process FIFO requirement. The mapping is stable for the process's
//! lifetime and dropped with [`LaneRouter::forget`] when the process is
//! removed, so the table cannot grow without bound.

use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Assigns pids to `lanes` lanes by round-robin on first sight.
#[derive(Debug)]
pub(crate) struct LaneRouter {
    lanes: usize,
    next: AtomicUsize,
    assigned: Mutex<HashMap<String, usize>>,
}

impl LaneRouter {
    /// A router over `lanes` lanes (a count below one is treated as one).
    pub(crate) fn new(lanes: usize) -> Self {
        Self {
            lanes: lanes.max(1),
            next: AtomicUsize::new(0),
            assigned: Mutex::new(HashMap::new()),
        }
    }

    /// The lane `pid` belongs to, in `0..lanes`. First sighting takes the
    /// next lane in turn; later calls return the same lane for the process's
    /// lifetime (or until [`Self::forget`]).
    pub(crate) fn lane_of(&self, pid: &str) -> usize {
        let mut assigned = self.assigned.lock();
        if let Some(&lane) = assigned.get(pid) {
            return lane;
        }
        let lane = self.next.fetch_add(1, Ordering::Relaxed) % self.lanes;
        assigned.insert(pid.to_string(), lane);
        lane
    }

    /// Drop the assignment of a removed process so the table cannot grow
    /// without bound. A late write for the forgotten pid re-registers it on
    /// the next lane in turn — harmless, because its rows are gone and such
    /// writes are skipped as dead.
    pub(crate) fn forget(&self, pid: &str) {
        self.assigned.lock().remove(pid);
    }

    /// Number of lanes.
    pub(crate) fn lanes(&self) -> usize {
        self.lanes
    }
}

/// Convenience for constructors that own their router.
pub(crate) fn shared(lanes: usize) -> Arc<LaneRouter> {
    Arc::new(LaneRouter::new(lanes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_spreads_pids_evenly_and_stays_stable() {
        let router = LaneRouter::new(4);
        let lanes: Vec<usize> = (0..16)
            .map(|i| router.lane_of(&format!("pid-{i}")))
            .collect();
        let mut counts = [0usize; 4];
        for lane in &lanes {
            counts[*lane] += 1;
        }
        assert!(
            counts.iter().all(|&count| count == 4),
            "expected an even spread, got {counts:?}"
        );
        // A hash mapping cannot make this guarantee: with fewer pids than
        // lanes it routinely leaves lanes empty forever.
        for (i, lane) in lanes.iter().enumerate() {
            assert_eq!(router.lane_of(&format!("pid-{i}")), *lane);
        }
    }

    #[test]
    fn forget_allows_the_pid_to_land_on_a_different_lane() {
        let router = LaneRouter::new(2);
        let first = router.lane_of("p1");
        router.lane_of("p2");
        router.lane_of("p3");
        router.forget("p1");
        assert_ne!(router.lane_of("p1"), first);
    }

    #[test]
    fn a_single_lane_is_always_lane_zero() {
        let router = LaneRouter::new(0);
        assert_eq!(router.lanes(), 1);
        assert_eq!(router.lane_of("any"), 0);
    }
}
