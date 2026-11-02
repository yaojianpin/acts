//! The RocksDB backend.
//!
//! Writes go through a rocksdb [`TransactionDB`](rocksdb::TransactionDB):
//! guard reads and the ops decided on them are one pessimistic transaction, so
//! a guarded batch is the same atomic unit the engine's collection layer and
//! [`DbLease`](acts::DbLease) rely on. A snapshot (`rocksdb::Snapshot`) serves
//! [`KvStore::read_stable`] and [`KvStore::read_view`], so a document family is
//! read at one committed version.
//!
//! Durability follows the same contract as the sled backend: a write is in
//! RocksDB's WAL before the call returns, and [`KvStore::flush`] is the
//! explicit barrier that flushes memtables and fsyncs the WAL. A process or
//! system crash can lose writes that were not yet fsynced; WAL records already
//! written are recovered by RocksDB on the next open.
//!
//! The write path is bounded by [`RocksdbOptions::max_in_flight_writes`]
//! (`WriteGate`). While every slot is occupied, `KvStore::saturated` is true;
//! the engine's store-writer uses it to stop admitting new work instead of
//! letting the RocksDB memtable/WAL stall propagate backwards as an
//! unbounded queue.

use crate::pressure::WriteGate;
use acts::{
    ActError, KvStore, Result, ScanOperation, ScanOptions, StorageUsage, StoreBatchOp, StoreGuard,
    StoreView,
};
use rocksdb::{
    DBCompressionType, Env, Options, ReadOptions, SingleThreaded, TransactionDB,
    TransactionDBOptions, WriteBatchWithTransaction,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Writes one physical RocksDB database admits at once by default. The engine
/// already runs one consumer per store-writer shard; this bound is the second
/// stage, in front of RocksDB's own write pipeline.
pub const DEFAULT_MAX_IN_FLIGHT_WRITES: usize = 16;

/// Memtable size per database by default (64 MiB). RocksDB flushes a memtable
/// to an L0 file when it fills; a larger buffer trades recovery replay for
/// fewer flushes, a smaller one the reverse.
pub const DEFAULT_WRITE_BUFFER_BYTES: usize = 64 * 1024 * 1024;

/// Tuning knobs for one RocksDB database. The defaults are sized for an
/// embedded engine database shared by several writer shards; an operator that
/// knows the working set can raise them without changing the backend.
#[derive(Clone, Debug)]
pub struct RocksdbOptions {
    /// Concurrent writes admitted per database (at least one).
    pub max_in_flight_writes: usize,
    /// Size of one memtable in bytes.
    pub write_buffer_size: usize,
    /// How many memtables may accumulate before a write is stalled.
    pub max_write_buffer_number: i32,
    /// Background flush/compaction jobs. `0` means the host's available
    /// parallelism (at least two).
    pub max_background_jobs: i32,
    /// Compress blocks with Snappy. The crate is built with the `snappy`
    /// feature; disabling this writes uncompressed blocks instead.
    pub compression: bool,
}

impl Default for RocksdbOptions {
    fn default() -> Self {
        Self {
            max_in_flight_writes: DEFAULT_MAX_IN_FLIGHT_WRITES,
            write_buffer_size: DEFAULT_WRITE_BUFFER_BYTES,
            max_write_buffer_number: 4,
            max_background_jobs: 0,
            compression: true,
        }
    }
}

/// An embedded RocksDB database implementing [`KvStore`].
pub struct RocksdbStore {
    db: Arc<TransactionDB<SingleThreaded>>,
    /// The directory [`KvStore::usage`] walks. `None` for the in-memory
    /// variant, which has no footprint on disk.
    path: Option<PathBuf>,
    /// The memory environment of the in-memory variant. Kept alive beside the
    /// database; the C handle is clone-counted, but owning it makes the
    /// lifetime explicit.
    _env: Option<Env>,
    gate: Arc<WriteGate>,
}

impl RocksdbStore {
    /// Open (or create) a database at `path` with the default options.
    pub fn open(path: &str) -> Result<Self> {
        Self::open_with(path, RocksdbOptions::default())
    }

    /// Open (or create) a database at `path` with explicit options.
    pub fn open_with(path: &str, options: RocksdbOptions) -> Result<Self> {
        let env = None;
        Self::open_configured(path, options, env, Some(PathBuf::from(path)))
    }

    /// An in-memory database. Tests and short-lived embedders use it; it has
    /// no file footprint, so [`KvStore::usage`] reports `None` bytes.
    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self> {
        let env = Env::mem_env().map_err(|e| ActError::Store(e.to_string()))?;
        Self::open_configured(
            "/acts-rocksdb-memory",
            RocksdbOptions::default(),
            Some(env),
            None,
        )
    }

    fn open_configured(
        path: &str,
        options: RocksdbOptions,
        env: Option<Env>,
        usage_path: Option<PathBuf>,
    ) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_write_buffer_size(options.write_buffer_size.max(1024 * 1024));
        opts.set_max_write_buffer_number(options.max_write_buffer_number.max(2));
        opts.set_max_background_jobs(if options.max_background_jobs > 0 {
            options.max_background_jobs
        } else {
            std::thread::available_parallelism()
                .map_or(2, |n| n.get() as i32)
                .max(2)
        });
        opts.set_compression_type(if options.compression {
            DBCompressionType::Snappy
        } else {
            DBCompressionType::None
        });
        opts.set_atomic_flush(true);
        opts.set_keep_log_file_num(8);
        if let Some(env) = &env {
            opts.set_env(env);
        }

        let mut txn_opts = TransactionDBOptions::new();
        // A guard key written continuously by another writer must surface as a
        // conflict, not park the caller forever. The engine's retry decides
        // what to do after `Ok(false)`.
        txn_opts.set_default_lock_timeout(5_000);

        let db = TransactionDB::open(&opts, &txn_opts, path)
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self {
            db: Arc::new(db),
            path: usage_path,
            _env: env,
            gate: WriteGate::new(options.max_in_flight_writes),
        })
    }

    fn map_err(error: rocksdb::Error) -> ActError {
        ActError::Store(error.to_string())
    }

    fn map_join(error: tokio::task::JoinError) -> ActError {
        ActError::Store(error.to_string())
    }
}

/// Sum every regular file's size under `dir`, recursively. Missing or
/// unreadable entries are skipped: the answer is the best-effort footprint of
/// a directory RocksDB may be compacting meanwhile.
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            total += dir_size(&entry.path());
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

/// The first key above every key that starts with `prefix`, or `None` when the
/// prefix has no finite successor (empty, or all `0xFF`). Used as the
/// exclusive upper bound of a prefix iteration.
fn prefix_upper_bound(prefix: &str) -> Option<Vec<u8>> {
    let mut bytes = prefix.as_bytes().to_vec();
    while let Some(last) = bytes.last_mut() {
        if *last == u8::MAX {
            bytes.pop();
        } else {
            *last += 1;
            return Some(bytes);
        }
    }
    None
}

/// Return true if `k` matches the scan operation given `key` and `prefix`.
fn key_matches(k: &str, key: &str, prefix: &str, op: &ScanOperation) -> bool {
    if !k.starts_with(prefix) {
        return false;
    }
    match op {
        ScanOperation::Eq => k.starts_with(key),
        ScanOperation::Ne => !k.starts_with(key),
        ScanOperation::In { values } => values.iter().any(|v| k.starts_with(v.as_str())),
        ScanOperation::Range { lower, upper } => {
            if let Some(l) = lower
                && k < l.as_str()
            {
                return false;
            }
            if let Some(u) = upper
                && k >= u.as_str()
            {
                return false;
            }
            true
        }
    }
}

/// The exclusive upper bound for one scan: the smaller of the prefix's
/// successor and the operation's own upper bound. `None` means "to the end of
/// the database".
fn scan_upper_bound(prefix: &str, op: &ScanOperation) -> Option<Vec<u8>> {
    let prefix_bound = prefix_upper_bound(prefix);
    let op_bound = match op {
        ScanOperation::Range { upper: Some(u), .. } => Some(u.as_bytes().to_vec()),
        _ => None,
    };
    match (prefix_bound, op_bound) {
        (Some(a), Some(b)) => Some(if a < b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

#[async_trait::async_trait]
impl KvStore for RocksdbStore {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || db.get(key.as_bytes()).map_err(Self::map_err))
            .await
            .map_err(Self::map_join)?
    }

    /// The database directory's footprint from a best-effort walk of its
    /// files. The in-memory variant reports no bytes.
    async fn usage(&self) -> Result<Option<StorageUsage>> {
        let Some(path) = self.path.clone() else {
            return Ok(Some(StorageUsage {
                kind: "rocksdb".to_string(),
                used_bytes: None,
            }));
        };
        let used = tokio::task::spawn_blocking(move || dir_size(&path))
            .await
            .map_err(Self::map_join)?;
        Ok(Some(StorageUsage {
            kind: "rocksdb".to_string(),
            used_bytes: Some(used),
        }))
    }

    /// Flush memtables and fsync the WAL: the explicit durability barrier.
    /// The engine calls it once at a graceful shutdown after the write-behind
    /// queue has drained.
    async fn flush(&self) -> Result<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            db.flush().map_err(Self::map_err)?;
            db.flush_wal(true).map_err(Self::map_err)
        })
        .await
        .map_err(Self::map_join)?
    }

    fn saturated(&self) -> bool {
        self.gate.saturated()
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let _permit = self.gate.acquire().await?;
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || db.put(key.as_bytes(), value).map_err(Self::map_err))
            .await
            .map_err(Self::map_join)?
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let _permit = self.gate.acquire().await?;
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || db.delete(key.as_bytes()).map_err(Self::map_err))
            .await
            .map_err(Self::map_join)?
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            db.multi_get(keys.iter().map(|key| key.as_bytes()))
                .into_iter()
                .map(|value| value.map_err(Self::map_err))
                .collect()
        })
        .await
        .map_err(Self::map_join)?
    }

    /// One RocksDB snapshot covers the whole key set: every value comes from
    /// the version the snapshot pinned, never a commit that landed after it.
    async fn read_stable(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            let snapshot = db.snapshot();
            let mut read_opts = ReadOptions::default();
            read_opts.set_snapshot(&snapshot);
            db.multi_get_opt(keys.iter().map(|key| key.as_bytes()), &read_opts)
                .into_iter()
                .map(|value| value.map_err(Self::map_err))
                .collect()
        })
        .await
        .map_err(Self::map_join)?
    }

    /// The key set and the prefix walk share one snapshot: a family that was
    /// committed in one batch is read as one version.
    async fn read_view(&self, keys: &[String], prefix: &str) -> Result<StoreView> {
        let db = self.db.clone();
        let keys = keys.to_vec();
        let prefix = prefix.to_owned();
        tokio::task::spawn_blocking(move || {
            let snapshot = db.snapshot();
            let mut read_opts = ReadOptions::default();
            read_opts.set_snapshot(&snapshot);
            if let Some(upper) = prefix_upper_bound(&prefix) {
                read_opts.set_iterate_upper_bound(upper);
            }
            let values = db
                .multi_get_opt(keys.iter().map(|key| key.as_bytes()), &read_opts)
                .into_iter()
                .map(|value| value.map_err(Self::map_err))
                .collect::<Result<Vec<_>>>()?;

            let mut entries = Vec::new();
            let mut iterator = db.raw_iterator_opt(read_opts);
            iterator.seek(prefix.as_bytes());
            while iterator.valid() {
                let (Some(key), Some(value)) = (iterator.key(), iterator.value()) else {
                    break;
                };
                if !key.starts_with(prefix.as_bytes()) {
                    break;
                }
                let key =
                    String::from_utf8(key.to_vec()).map_err(|e| ActError::Store(e.to_string()))?;
                entries.push((key, value.to_vec()));
                iterator.next();
            }
            iterator.status().map_err(Self::map_err)?;
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Ok(StoreView { values, entries })
        })
        .await
        .map_err(Self::map_join)?
    }

    /// One pessimistic transaction for the guard reads and the ops: a guard
    /// mismatch applies nothing, and two writers racing the same guard cannot
    /// both commit it. A guardless batch uses one `WriteBatch`, so its ops
    /// still become visible together.
    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if ops.is_empty() && guards.is_empty() {
            return Ok(true);
        }
        let _permit = self.gate.acquire().await?;
        let db = self.db.clone();
        let ops = ops.to_vec();
        let guards = guards.to_vec();
        tokio::task::spawn_blocking(move || {
            if guards.is_empty() {
                if ops.len() == 1 {
                    return match &ops[0] {
                        StoreBatchOp::Put { key, value } => {
                            db.put(key.as_bytes(), value).map(|()| true)
                        }
                        StoreBatchOp::Delete { key } => db.delete(key.as_bytes()).map(|()| true),
                    }
                    .map_err(Self::map_err);
                }
                let mut batch = WriteBatchWithTransaction::<true>::default();
                for op in &ops {
                    match op {
                        StoreBatchOp::Put { key, value } => batch.put(key.as_bytes(), value),
                        StoreBatchOp::Delete { key } => batch.delete(key.as_bytes()),
                    }
                }
                return db.write(batch).map(|()| true).map_err(Self::map_err);
            }

            let transaction = db.transaction();
            for guard in &guards {
                // Exclusive, so the comparison below is against bytes no
                // other transaction can move until this one commits or drops.
                let stored = transaction
                    .get_for_update(guard.key.as_bytes(), true)
                    .map_err(Self::map_err)?;
                if !guard.matches(stored.as_deref()) {
                    // Dropping the transaction rolls its locks back; no op
                    // was written.
                    return Ok(false);
                }
            }
            for op in &ops {
                match op {
                    StoreBatchOp::Put { key, value } => transaction
                        .put(key.as_bytes(), value)
                        .map_err(Self::map_err)?,
                    StoreBatchOp::Delete { key } => {
                        transaction.delete(key.as_bytes()).map_err(Self::map_err)?
                    }
                }
            }
            transaction.commit().map(|()| true).map_err(Self::map_err)
        })
        .await
        .map_err(Self::map_join)?
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            let ScanOptions {
                is_rev,
                op,
                ref prefix,
            } = options;
            let mut read_opts = ReadOptions::default();
            if let Some(upper) = scan_upper_bound(prefix, &op) {
                read_opts.set_iterate_upper_bound(upper.clone());
            }
            let mut result = Vec::new();
            let mut iterator = db.raw_iterator_opt(read_opts);

            if is_rev {
                let end = scan_upper_bound(prefix, &op);
                match end {
                    Some(end) => {
                        iterator.seek_for_prev(&end);
                        // `seek_for_prev` is inclusive; the bound is not.
                        if iterator.valid()
                            && iterator
                                .key()
                                .is_some_and(|stored| stored >= end.as_slice())
                        {
                            iterator.prev();
                        }
                    }
                    None => iterator.seek_to_last(),
                }
            } else {
                let start = match &op {
                    ScanOperation::Range {
                        lower: Some(lower), ..
                    } if lower.starts_with(prefix.as_str()) => lower.as_bytes(),
                    _ => prefix.as_bytes(),
                };
                iterator.seek(start);
            }

            while iterator.valid() {
                let (Some(stored_key), Some(value)) = (iterator.key(), iterator.value()) else {
                    break;
                };
                let Ok(stored_key) = std::str::from_utf8(stored_key) else {
                    return Err(ActError::Store("store key is not UTF-8".to_string()));
                };
                if !stored_key.starts_with(prefix.as_str()) {
                    break;
                }
                if let ScanOperation::Range {
                    lower: _,
                    upper: Some(upper),
                } = &op
                    && stored_key >= upper.as_str()
                {
                    break;
                }
                // A lower bound only stops a reverse walk: descending from the
                // upper end, everything below it is outside the interval.
                if is_rev
                    && let ScanOperation::Range {
                        lower: Some(lower), ..
                    } = &op
                    && stored_key < lower.as_str()
                {
                    break;
                }
                if key_matches(stored_key, &key, prefix, &op) {
                    result.push((stored_key.to_string(), value.to_vec()));
                }
                if is_rev {
                    iterator.prev();
                } else {
                    iterator.next();
                }
            }
            iterator.status().map_err(Self::map_err)?;
            Ok(result)
        })
        .await
        .map_err(Self::map_join)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_upper_bound_is_exclusive_and_finite() {
        assert_eq!(prefix_upper_bound("abc"), Some(b"abd".to_vec()));
        assert_eq!(prefix_upper_bound(""), None);
    }

    #[test]
    fn scan_upper_bound_takes_the_narrower_bound() {
        let op = ScanOperation::Range {
            lower: None,
            upper: Some("tasks-z".to_string()),
        };
        assert_eq!(
            scan_upper_bound("tasks-", &op),
            Some(b"tasks-z".to_vec()),
            "the operation bound is below the prefix successor"
        );
        assert_eq!(
            scan_upper_bound("tasks-", &ScanOperation::Eq),
            Some(b"tasks.".to_vec())
        );
    }
}
