//! The one pid → lane mapping, shared by every component that keeps one
//! process's work ordered while independent processes run concurrently: the
//! scheduler task lanes, the per-process event gate and the store writer
//! shards. Sharing it means the routing can never disagree — and a component
//! that must keep its work serialized against another's (scheduler jobs
//! against the event handlers of the same process) gets it for free.

/// Index of the lane `pid` belongs to, in `0..lanes` (`lanes` is never 0; a
/// count below one is treated as one). FNV-1a over the pid, so the same pid
/// always maps to the same lane within a process.
pub(crate) fn pid_lane(pid: &str, lanes: usize) -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in pid.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash as usize) % lanes.max(1)
}
