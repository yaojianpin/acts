//! The one pid → lane mapping, shared by every component that keeps one
//! process's work ordered while independent processes run concurrently: the
//! scheduler's worker pool and the per-process event gate share the same
//! function, and the store writer shards run their own instance over its own
//! lane count. Because the mapping is a pure function of the pid, separate
//! instances with the same lane count always agree — and a component that must
//! keep its work serialized against another's (scheduler jobs against the
//! event handlers of the same process) gets it for free.
//!
//! Assignment is XXH3-64 over the pid bytes, modulo the lane count. The hash
//! is seedless and stable across processes and restarts, so a run lands on the
//! same lane everywhere and the router keeps no per-pid table — there is
//! nothing to grow and nothing to forget when a process is removed. Over a
//! large live set the lanes are evenly loaded; with fewer live pids than lanes
//! some lanes stay idle (the price of a stable mapping), and a single hot
//! process still pins its lane (the price of the per-process FIFO
//! requirement).

use std::sync::Arc;

use xxhash_rust::xxh3::xxh3_64;

/// Assigns pids to `lanes` lanes by their XXH3-64 hash.
#[derive(Debug)]
pub(crate) struct LaneRouter {
    lanes: usize,
}

impl LaneRouter {
    /// A router over `lanes` lanes (a count below one is treated as one).
    pub(crate) fn new(lanes: usize) -> Self {
        Self {
            lanes: lanes.max(1),
        }
    }

    /// The lane `pid` belongs to, in `0..lanes`. A pure function of the pid
    /// and the lane count: every call — and every router instance with the
    /// same count — returns the same lane for the same process.
    pub(crate) fn lane_of(&self, pid: &str) -> usize {
        (xxh3_64(pid.as_bytes()) % self.lanes as u64) as usize
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
    fn a_pid_keeps_its_lane_and_independent_routers_agree() {
        let first = LaneRouter::new(8);
        let second = LaneRouter::new(8);
        for pid in ["p-1", "p-2", "p-3", "process-with-a-long-name"] {
            assert_eq!(first.lane_of(pid), first.lane_of(pid));
            assert_eq!(first.lane_of(pid), second.lane_of(pid));
            assert!(first.lane_of(pid) < 8);
        }
    }

    #[test]
    fn the_hash_spreads_many_pids_evenly_across_lanes() {
        let router = LaneRouter::new(8);
        let pids = 16_384usize;
        let mut counts = [0usize; 8];
        for index in 0..pids {
            counts[router.lane_of(&format!("pid-{index}"))] += 1;
        }
        let expected = pids / 8;
        for (lane, &count) in counts.iter().enumerate() {
            // A deterministic hash, not a live round-robin: allow statistical
            // skew, but every lane must carry its share and stay near it.
            assert!(
                count > expected / 2 && count < expected * 3 / 2,
                "lane {lane} carries {count} of {pids}: {counts:?}"
            );
        }
    }

    #[test]
    fn a_single_lane_is_always_lane_zero() {
        let router = LaneRouter::new(0);
        assert_eq!(router.lanes(), 1);
        assert_eq!(router.lane_of("any"), 0);
    }
}
