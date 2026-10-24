//! The snapshot TTL purge: drops expired cache entries so never-read,
//! never-tombstoned scopes cannot grow the cache forever. The sweep's work is
//! in-memory, so unlike the store-facing loops it keeps no failure guard.

use tracing::debug;

use super::TimerGuider;

impl TimerGuider {
    /// One snapshot TTL sweep pass.
    pub(crate) async fn purge_tick(&self) {
        let removed = self.runtime.snapshot_registry().purge_expired();
        if removed > 0 {
            debug!(removed, "expired snapshot entries purged");
        }
    }
}
