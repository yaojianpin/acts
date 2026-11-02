//! External [`KvStore`](acts::KvStore) backends for the acts workflow engine.
//!
//! Each backend implements the [`KvStore`](acts::KvStore) trait defined by the
//! `acts` crate and can be handed to
//! [`EngineBuilder::set_store`] the same way as [`MemoryStore`](acts::MemoryStore):
//!
//! ```rust,ignore
//! use acts::Engine;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> acts::Result<()> {
//!     let store = acts_store::SqliteStore::open("data/acts.db").await?;
//!     let engine = Engine::builder()
//!         .set_store(Arc::new(store))
//!         .start()
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! - [`SqliteStore`] — feature `sqlite`
//! - [`PostgresStore`] — feature `postgres`
//! - [`SledStore`] — feature `sled`
//! - [`RocksdbStore`] — feature `rocksdb`
//! - [`ShardedStore`] — no feature; wraps several backends behind one
//!   consistent-hash shard ring
//!
//! SQLite and Postgres make a commit durable before it returns. `SledStore`
//! applies writes immediately and is flushed by sled's background flusher
//! every [`DEFAULT_FLUSH_EVERY_MS`] (50 ms); `RocksdbStore` writes the WAL
//! before returning and flushes memtables/WAL on [`KvStore::flush`]. A crash
//! can therefore lose writes applied since the last completed flush on either
//! embedded backend; the engine performs one explicit barrier at a graceful
//! shutdown.
//!
//! Every backend here commits a batch with its guards (see
//! [`KvStore::batch`](acts::KvStore::batch)), which is what
//! [`DbLease`](acts::DbLease) needs to hold a database exclusively across
//! processes.
//!
//! # Sharding
//!
//! [`ShardedStore`] spreads the engine's write load over several physical
//! databases. Process-owned keys route by pid through a 1024-slot consistent
//! hash ring ([`DEFAULT_VIRTUAL_NODES`]); scans fan out and merge by key, so
//! ordered queries stay globally ordered. `ShardedStore::rebalance` moves rows
//! after a shard is added or removed, and [`Shard::draining`] keeps a removed
//! shard readable until its rows have moved. A guarded batch must stay on one
//! physical shard: the database lease is a global row, so a fenced store over
//! several shards is not supported — run one server per shard set with
//! `lease = false`, or coordinate writers outside the engine. Each shard
//! bounds its write pipeline and reports it through `KvStore::saturated`,
//! which the engine's store-writer combines with its own backlog.

mod consts;
mod pressure;

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "rocksdb")]
mod rocksdb;
mod shard;
#[cfg(feature = "sled")]
mod sled;
#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
#[cfg(feature = "rocksdb")]
pub use rocksdb::{
    DEFAULT_MAX_IN_FLIGHT_WRITES, DEFAULT_WRITE_BUFFER_BYTES, RocksdbOptions, RocksdbStore,
};
pub use shard::{
    DEFAULT_SHARD_WRITES, DEFAULT_VIRTUAL_NODES, RebalanceReport, Shard, ShardConfig, ShardRole,
    ShardedStore,
};
#[cfg(feature = "sled")]
pub use sled::{DEFAULT_FLUSH_EVERY_MS, SledStore};
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

#[cfg(test)]
mod snapshot_view_tests {
    #[cfg(feature = "rocksdb")]
    use crate::RocksdbStore;
    #[cfg(feature = "sled")]
    use crate::SledStore;
    #[cfg(feature = "sqlite")]
    use crate::SqliteStore;
    use acts::{KvStore, MemoryStore, StoreBatchOp};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// One family's keys: head, the three derived indexes, the base, and a
    /// two-entry log window under the head's prefix.
    #[derive(Clone)]
    struct Family {
        keys: Vec<String>,
        log_prefix: String,
        log_keys: [String; 2],
    }

    impl Family {
        fn new(lane: usize) -> Self {
            let root = format!("stress/{lane}/head");
            let log_prefix = format!("{root}/log/");
            Self {
                keys: vec![
                    root.clone(),
                    format!("stress/{lane}/rec"),
                    format!("stress/{lane}/out"),
                    format!("stress/{lane}/ready"),
                    format!("{root}/base"),
                ],
                log_keys: [
                    format!("{log_prefix}00000000000000000001"),
                    format!("{log_prefix}00000000000000000002"),
                ],
                log_prefix,
            }
        }

        /// The one batch a commit writes: every document carries the same
        /// version marker.
        fn commit_ops(&self, version: usize) -> Vec<StoreBatchOp> {
            let bytes = format!("v{version:08}").into_bytes();
            let mut ops = self
                .keys
                .iter()
                .map(|key| StoreBatchOp::Put {
                    key: key.clone(),
                    value: bytes.clone(),
                })
                .collect::<Vec<_>>();
            for key in &self.log_keys {
                ops.push(StoreBatchOp::Put {
                    key: key.clone(),
                    value: bytes.clone(),
                });
            }
            ops
        }

        /// The cleanup batch: every document of the family in one step.
        fn reclaim_ops(&self) -> Vec<StoreBatchOp> {
            self.keys
                .iter()
                .chain(self.log_keys.iter())
                .map(|key| StoreBatchOp::Delete { key: key.clone() })
                .collect()
        }
    }

    async fn assert_one_version(store: &dyn KvStore, family: &Family) {
        let view = store
            .read_view(&family.keys, &family.log_prefix)
            .await
            .expect("a stable view");
        match &view.values[0] {
            None => {
                assert!(
                    view.values.iter().all(Option::is_none),
                    "a cleanup removed part of a family: {:?}",
                    view.values
                );
                assert!(
                    view.entries.is_empty(),
                    "log entries survived their head: {:?}",
                    view.entries
                );
            }
            Some(version) => {
                for (position, value) in view.values.iter().enumerate() {
                    assert_eq!(
                        value.as_deref(),
                        Some(version.as_slice()),
                        "key {position} came from another version"
                    );
                }
                assert_eq!(
                    view.entries.len(),
                    family.log_keys.len(),
                    "the log window must be part of the same view"
                );
                for (_, entry) in &view.entries {
                    assert_eq!(entry, version, "a log entry came from another version");
                }
                assert!(
                    view.entries.windows(2).all(|pair| pair[0].0 < pair[1].0),
                    "the view's entries are key-ordered"
                );
            }
        }
    }

    /// One writer per lane commits its family in one batch, lane 0 is also
    /// cleaned up and re-created periodically, and one reader samples lane 0
    /// for the whole run. Every view must be exactly one version: absent as a
    /// whole, or all documents and the whole log window carrying one marker.
    ///
    /// This is the primitive the durable load is built on; `acts`' own load
    /// tests cover the repository above it.
    async fn exercise(store: Arc<dyn KvStore>, lanes: usize, commits_per_lane: usize) {
        let families = (0..lanes).map(Family::new).collect::<Vec<_>>();
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicUsize::new(0));
        let views = Arc::new(AtomicUsize::new(0));
        let mut writers = Vec::with_capacity(lanes);
        for (lane, family) in families.iter().cloned().enumerate() {
            let writer = {
                let store = store.clone();
                let done = done.clone();
                let finished = finished.clone();
                tokio::spawn(async move {
                    for version in 0..commits_per_lane {
                        store
                            .batch(&family.commit_ops(version), &[])
                            .await
                            .expect("a commit");
                        if lane == 0 && version % 7 == 0 {
                            store
                                .batch(&family.reclaim_ops(), &[])
                                .await
                                .expect("a cleanup");
                        }
                    }
                    if finished.fetch_add(1, Ordering::SeqCst) + 1 == lanes {
                        done.store(true, Ordering::SeqCst);
                    }
                })
            };
            writers.push(writer);
        }
        let target = families[0].clone();
        let reader = {
            let store = store.clone();
            let done = done.clone();
            let views = views.clone();
            tokio::spawn(async move {
                while !done.load(Ordering::SeqCst) {
                    assert_one_version(store.as_ref(), &target).await;
                    views.fetch_add(1, Ordering::Relaxed);
                }
                for _ in 0..200 {
                    assert_one_version(store.as_ref(), &target).await;
                    views.fetch_add(1, Ordering::Relaxed);
                }
            })
        };
        for writer in writers {
            writer.await.unwrap();
        }
        reader.await.unwrap();
        assert!(
            views.load(Ordering::Relaxed) > 200,
            "lanes={lanes}: the reader must have sampled the whole write run"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn memory_views_are_one_version_under_concurrent_writers() {
        for lanes in [1usize, 4, 70] {
            exercise(Arc::new(MemoryStore::new()), lanes, (6_000 / lanes).max(50)).await;
        }
    }

    #[cfg(feature = "sled")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn sled_views_are_one_version_under_concurrent_writers() {
        for lanes in [1usize, 4, 70] {
            let store = SledStore::open_in_memory().expect("sled");
            exercise(Arc::new(store), lanes, (6_000 / lanes).max(50)).await;
        }
    }

    #[cfg(feature = "rocksdb")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn rocksdb_views_are_one_version_under_concurrent_writers() {
        for lanes in [1usize, 4, 70] {
            let store = RocksdbStore::open_in_memory().expect("rocksdb");
            exercise(Arc::new(store), lanes, (2_000 / lanes).max(30)).await;
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn sqlite_views_are_one_version_under_concurrent_writers() {
        for lanes in [1usize, 4, 70] {
            let path = std::env::temp_dir().join(format!(
                "acts-view-{}-{}.sqlite",
                std::process::id(),
                lanes
            ));
            let store = SqliteStore::open(&path.display().to_string())
                .await
                .expect("sqlite");
            exercise(Arc::new(store), lanes, (2_000 / lanes).max(30)).await;
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
            let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        }
    }

    /// A single-key read is intentionally not a view; the stable read of
    /// several keys is. Before a family is written, the view is all-absent
    /// rather than an error.
    #[tokio::test]
    async fn an_empty_family_reads_as_absent() {
        let store = MemoryStore::new();
        let family = Family::new(0);
        let view = store
            .read_view(&family.keys, &family.log_prefix)
            .await
            .unwrap();
        assert!(view.values.iter().all(Option::is_none));
        assert!(view.entries.is_empty());
        assert_eq!(
            store.read_stable(&family.keys).await.unwrap().len(),
            family.keys.len()
        );
    }
}
