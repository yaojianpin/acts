//! The sled backend.
//!
//! Writes are applied to sled's in-memory log and are visible to readers
//! immediately. They are made durable by sled's background flusher every
//! [`DEFAULT_FLUSH_EVERY_MS`] (50 ms), not by a flush per write: every writer
//! shares one `sled::Db` and one log/fsync barrier, so a per-write flush made
//! all writer shards queue behind the same disk operation.
//!
//! The acknowledged-write window this leaves is explicit: a process or system
//! crash can lose the writes applied since the last completed background
//! flush (up to the window, longer if the flusher is delayed). [`KvStore::flush`]
//! is the explicit barrier, and the engine calls it once after draining the
//! write-behind queue at a graceful shutdown. An embedder that needs a
//! stronger acknowledgement boundary can call it directly.

use acts::{
    ActError, KvStore, Result, ScanOperation, ScanOptions, StorageUsage, StoreBatchOp, StoreGuard,
    StoreView,
};
use sled::transaction::{ConflictableTransactionError, TransactionError};
use std::path::Path;
use std::sync::Arc;

/// How many times sled may re-run a guarded batch's closure before the batch
/// is reported as a conflict instead. sled retries on its own whenever a key
/// the transaction read changed, so this bound only matters when the guard key
/// is written continuously — a guard that keeps changing cannot be proven held.
const BATCH_MAX_ATTEMPTS: usize = 32;

/// The background durability cadence when `SledStore::open` does not name one.
/// A write is visible to readers immediately and is made durable by sled's
/// periodic flusher; this window is therefore the maximum data a crash can
/// lose without an explicit [`KvStore::flush`] barrier. `SledStore` no longer
/// fsyncs on every write: eight writer shards sharing one database would all
/// queue on that single barrier.
pub const DEFAULT_FLUSH_EVERY_MS: u64 = 50;

/// Sum every regular file's size under `dir`, recursively. Missing or
/// unreadable entries are skipped: the answer is the best-effort footprint of
/// a directory another writer may be compacting meanwhile.
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

pub struct SledStore {
    db: sled::Db,
    /// The directory the database was opened over — what [`KvStore::usage`]
    /// walks. `None` for the in-memory variant, which has no footprint on
    /// disk to measure (sled's `Db` does not expose its path itself).
    path: Option<std::path::PathBuf>,
    /// Serializes a multi-document read against writes (see
    /// [`KvStore::read_view`]). sled 0.34 transactions read point keys only —
    /// they cannot walk a prefix — so the view's prefix scan cannot join the
    /// same transaction as its key reads. Holding the write side of this lock
    /// in every mutation makes the read side one stable version without a
    /// retry loop, which is what a load needs while a writer commits the same
    /// process. sled allows one process per database directory, so this lock
    /// sees every writer.
    barrier: Arc<std::sync::RwLock<()>>,
}

impl SledStore {
    pub fn open(path: &str) -> Result<Self> {
        let db = sled::Config::new()
            .path(path)
            .flush_every_ms(Some(DEFAULT_FLUSH_EVERY_MS))
            .open()
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self {
            db,
            path: Some(std::path::PathBuf::from(path)),
            barrier: Arc::new(std::sync::RwLock::new(())),
        })
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self> {
        let db = sled::Config::new()
            .temporary(true)
            .flush_every_ms(Some(DEFAULT_FLUSH_EVERY_MS))
            .open()
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self {
            db,
            path: None,
            barrier: Arc::new(std::sync::RwLock::new(())),
        })
    }
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

#[async_trait::async_trait]
impl KvStore for SledStore {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            db.get(key.as_bytes())
                .map(|opt| opt.map(|ivec| ivec.to_vec()))
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
        .map_err(|e| ActError::Store(e.to_string()))
    }

    /// The bytes the database's directory holds on disk: a walk of the
    /// directory the store was opened over, summing every file's size (sled's
    /// log files, compression blobs and the configs it keeps beside them). A
    /// file that disappears or becomes unreadable between the listing and the
    /// stat is skipped — the walk is best effort, and a concurrent compaction
    /// must not fail a status read. The in-memory variant answers `None`: it
    /// has no footprint on disk to measure.
    async fn usage(&self) -> Result<Option<StorageUsage>> {
        let Some(path) = self.path.clone() else {
            return Ok(Some(StorageUsage {
                kind: "sled".to_string(),
                used_bytes: None,
            }));
        };
        let used = tokio::task::spawn_blocking(move || dir_size(&path))
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Some(StorageUsage {
            kind: "sled".to_string(),
            used_bytes: Some(used),
        }))
    }

    /// The explicit durability barrier: rolls sled's in-memory log and fsyncs
    /// it. The engine calls this once at a graceful shutdown; a caller that
    /// needs an acknowledgement boundary stronger than the background
    /// `flush_every_ms` window can call it directly.
    async fn flush(&self) -> Result<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.flush())
            .await
            .map_err(|e| ActError::Store(e.to_string()))?
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    /// Every key read under one read guard of the write barrier: no mutation
    /// can land between two values, so the answer is one committed version.
    async fn read_stable(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        let db = self.db.clone();
        let barrier = self.barrier.clone();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            let _view = barrier
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            keys.iter()
                .map(|key| {
                    db.get(key.as_bytes())
                        .map(|value| value.map(|bytes| bytes.to_vec()))
                })
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| ActError::Store(e.to_string()))
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }

    /// The keys and the prefix walk are read under the same read guard: a
    /// commit can land before or after the view, never inside it. sled 0.34's
    /// transactions cannot walk a prefix, so the barrier — not a retry loop —
    /// is what makes the two parts one version.
    async fn read_view(&self, keys: &[String], prefix: &str) -> Result<StoreView> {
        let db = self.db.clone();
        let barrier = self.barrier.clone();
        let keys = keys.to_vec();
        let prefix = prefix.to_owned();
        tokio::task::spawn_blocking(move || {
            let _view = barrier
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let values = keys
                .iter()
                .map(|key| {
                    db.get(key.as_bytes())
                        .map(|value| value.map(|bytes| bytes.to_vec()))
                })
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| ActError::Store(e.to_string()))?;
            let mut entries = Vec::new();
            for entry in db.scan_prefix(prefix.as_bytes()) {
                let (key, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                let key =
                    String::from_utf8(key.to_vec()).map_err(|e| ActError::Store(e.to_string()))?;
                if key.starts_with(prefix.as_str()) {
                    entries.push((key, value.to_vec()));
                }
            }
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Ok(StoreView { values, entries })
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let db = self.db.clone();
        let barrier = self.barrier.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            let _write = barrier
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            db.insert(key.as_bytes(), value)
                .map(|_| ())
                .map_err(|e| ActError::Store(e.to_string()))
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let db = self.db.clone();
        let barrier = self.barrier.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            let _write = barrier
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            db.remove(key.as_bytes())
                .map(|_| ())
                .map_err(|e| ActError::Store(e.to_string()))
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let db = self.db.clone();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            keys.into_iter()
                .map(|key| {
                    db.get(key.as_bytes())
                        .map(|opt| opt.map(|ivec| ivec.to_vec()))
                })
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| ActError::Store(e.to_string()))
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if ops.is_empty() && guards.is_empty() {
            return Ok(true);
        }
        let db = self.db.clone();
        let barrier = self.barrier.clone();
        let ops = ops.to_vec();
        let guards = guards.to_vec();
        tokio::task::spawn_blocking(move || {
            // A batch is one version change: a view either sees all of it or
            // none of it.
            let _write = barrier
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guards.is_empty() {
                let mut sled_batch = sled::Batch::default();
                for op in ops {
                    match op {
                        StoreBatchOp::Put { key, value } => {
                            sled_batch.insert(key.as_bytes(), value);
                        }
                        StoreBatchOp::Delete { key } => {
                            sled_batch.remove(key.as_bytes());
                        }
                    }
                }
                db.apply_batch(sled_batch)
                    .map_err(|e| ActError::Store(e.to_string()))?;
                return Ok(true);
            }

            // `Transaction` is the backend's serializable multi-key unit: the
            // guard reads below and the ops are one transaction, and sled
            // re-runs the closure whenever a key it read changed under it, so
            // a guard read can never be stale at commit time. A conflict that
            // keeps repeating (the guard key written continuously by another
            // writer) is bounded here instead of retrying forever.
            let attempts = std::cell::Cell::new(0usize);
            let outcome = db.transaction(|tx| {
                if attempts.replace(attempts.get() + 1) > BATCH_MAX_ATTEMPTS {
                    return Err(ConflictableTransactionError::Abort(false));
                }
                for guard in &guards {
                    let stored = tx.get(guard.key.as_bytes())?;
                    if !guard.matches(stored.as_ref().map(|value| value.as_ref())) {
                        return Err(ConflictableTransactionError::Abort(false));
                    }
                }
                for op in &ops {
                    match op {
                        StoreBatchOp::Put { key, value } => {
                            tx.insert(key.as_bytes(), value.clone())?;
                        }
                        StoreBatchOp::Delete { key } => {
                            tx.remove(key.as_bytes())?;
                        }
                    }
                }
                Ok(true)
            });
            let applied = match outcome {
                Ok(applied) => applied,
                // Our own guard mismatch (or the retry bound), reported
                // through the abort channel.
                Err(TransactionError::Abort(applied)) => applied,
                Err(TransactionError::Storage(err)) => {
                    return Err(ActError::Store(err.to_string()));
                }
            };
            Ok(applied)
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
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
            let mut result = Vec::new();
            match &op {
                // A value-key prefix is itself a valid (and much narrower) sled
                // prefix for Eq.
                ScanOperation::Eq if key.starts_with(prefix.as_str()) => {
                    for entry in db.scan_prefix(key.as_bytes()) {
                        let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                        let key_str = String::from_utf8(k.to_vec())
                            .map_err(|e| ActError::Store(e.to_string()))?;
                        result.push((key_str, value.to_vec()));
                    }
                }
                ScanOperation::Range { lower, upper } => {
                    let start = lower.as_ref().filter(|l| l.starts_with(prefix.as_str()));
                    let iterator = match start {
                        Some(lower) => db.range(lower.as_bytes()..),
                        None => db.scan_prefix(prefix.as_bytes()),
                    };
                    for entry in iterator {
                        let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                        let key_str = String::from_utf8(k.to_vec())
                            .map_err(|e| ActError::Store(e.to_string()))?;
                        if !key_str.starts_with(prefix.as_str()) {
                            break;
                        }
                        if let Some(upper) = upper
                            && key_str.as_str() >= upper.as_str()
                        {
                            break;
                        }
                        if key_matches(&key_str, &key, prefix, &op) {
                            result.push((key_str, value.to_vec()));
                        }
                    }
                }
                _ => {
                    for entry in db.scan_prefix(prefix.as_bytes()) {
                        let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                        let key_str = String::from_utf8(k.to_vec())
                            .map_err(|e| ActError::Store(e.to_string()))?;
                        if key_matches(&key_str, &key, prefix, &op) {
                            result.push((key_str, value.to_vec()));
                        }
                    }
                }
            }
            if is_rev {
                result.reverse();
            }
            Ok(result)
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }
}
