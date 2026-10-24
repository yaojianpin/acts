//! The snapshot TTL purge: periodically drops expired cache entries so
//! never-read, never-tombstoned scopes cannot grow the cache forever. The
//! sweep's work is in-memory, so unlike the store-facing loops it keeps no
//! failure guard.

use std::time::Duration;

use tokio::time;
use tracing::debug;

use super::TimerGuider;

impl TimerGuider {
    /// Snapshot TTL sweep — periodically drops expired cache entries.
    pub(crate) fn purge_timer(&self) {
        let config = self.runtime.config();
        let interval_ms = super::loop_interval(
            config.data.snapshot_interval_secs,
            config.snapshot_interval_secs(),
            super::tick_interval(config),
        )
        .as_millis() as u64;

        let registry = self.runtime.snapshot_registry();
        let shutdown = self.runtime.shutdown_token();
        tokio::spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                let removed = registry.purge_expired();
                if removed > 0 {
                    debug!(removed, "expired snapshot entries purged");
                }
            }
        });
    }
}
