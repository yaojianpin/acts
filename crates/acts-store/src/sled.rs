use acts::{ActError, KvStore, Result, ScanOperation, ScanOptions, StoreBatchOp, StoreGuard};
use sled::transaction::{ConflictableTransactionError, TransactionError};

/// How many times sled may re-run a guarded batch's closure before the batch
/// is reported as a conflict instead. sled retries on its own whenever a key
/// the transaction read changed, so this bound only matters when the guard key
/// is written continuously — a guard that keeps changing cannot be proven held.
const BATCH_MAX_ATTEMPTS: usize = 32;

pub struct SledStore {
    db: sled::Db,
}

impl SledStore {
    pub fn open(path: &str) -> Result<Self> {
        let db = sled::open(path).map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { db })
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self> {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { db })
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

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            db.insert(key.as_bytes(), value)
                .map_err(|e| ActError::Store(e.to_string()))?;
            // Preserve the existing per-write durability guarantee while
            // keeping the sled call and its flush off the async worker.
            db.flush()
                .map(|_| ())
                .map_err(|e| ActError::Store(e.to_string()))
        })
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let db = self.db.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            db.remove(key.as_bytes())
                .map_err(|e| ActError::Store(e.to_string()))?;
            db.flush()
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
        let ops = ops.to_vec();
        let guards = guards.to_vec();
        tokio::task::spawn_blocking(move || {
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
                // One durability boundary belongs to the atomic batch itself.
                db.flush().map_err(|e| ActError::Store(e.to_string()))?;
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
            if applied {
                // The transaction's durability boundary, as the plain batch has.
                db.flush().map_err(|e| ActError::Store(e.to_string()))?;
            }
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
