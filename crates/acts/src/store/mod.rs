mod collection;
pub mod data;
mod memory;
pub mod query;

#[cfg(feature = "store-nats")]
mod nats;
#[cfg(feature = "store-postgres")]
mod postgres;
#[cfg(feature = "store-redis")]
mod redis;
#[cfg(feature = "store-sled")]
mod sled;
#[cfg(feature = "store-sqlite")]
mod sqlite;

#[allow(clippy::module_inception)]
mod store;

#[cfg(test)]
mod tests;

use data::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
#[allow(unused_imports)]
pub use store::Store;

use crate::{ActError, Result};
use query::*;
use std::error::Error;
use strum::{AsRefStr, EnumIter};

#[allow(unused_imports)]
pub use memory::MemoryStore;

#[cfg(feature = "store-nats")]
#[allow(unused_imports)]
pub use nats::NatsStore;
#[cfg(feature = "store-postgres")]
#[allow(unused_imports)]
pub use postgres::PostgresStore;
#[cfg(feature = "store-redis")]
#[allow(unused_imports)]
pub use redis::RedisStore;
#[cfg(feature = "store-sled")]
#[allow(unused_imports)]
pub use sled::SledStore;
#[cfg(feature = "store-sqlite")]
#[allow(unused_imports)]
pub use sqlite::SqliteStore;

fn map_db_err(err: impl Error) -> ActError {
    ActError::Store(err.to_string())
}

#[derive(Debug, Clone, AsRefStr, PartialEq, Hash, Eq, EnumIter)]
pub enum StoreIden {
    #[strum(serialize = "packages")]
    Packages,
    #[strum(serialize = "models")]
    Models,
    #[strum(serialize = "procs")]
    Procs,
    #[strum(serialize = "tasks")]
    Tasks,
    #[strum(serialize = "messages")]
    Messages,
    #[strum(serialize = "deliveries")]
    Deliveries,
    #[strum(serialize = "events")]
    Events,
    #[strum(serialize = "ops")]
    Ops,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PageData<T> {
    pub count: usize,
    pub page_num: usize,
    pub page_count: usize,
    pub page_size: usize,
    pub rows: Vec<T>,
}

pub trait DbCollectionIden {
    fn iden() -> StoreIden;
    fn indexed_fields() -> &'static [&'static str] {
        &[]
    }
    fn version() -> i32 {
        0
    }

    /// Default deserialization for the current version — can be called by
    /// overriding `upcast` impls when the version matches [`Self::version()`].
    fn upcast_current(mut value: JsonValue) -> Result<Self>
    where
        Self: DeserializeOwned,
    {
        // Ensure v field exists for backward compatibility with older records
        if let JsonValue::Object(ref mut map) = value {
            map.entry("v".to_string())
                .or_insert_with(|| JsonValue::Number(serde_json::Number::from(0)));
        }
        serde_json::from_value(value).map_err(map_db_err)
    }

    fn upcast(value: JsonValue) -> Result<Self>
    where
        Self: DeserializeOwned,
    {
        Self::upcast_current(value)
    }
}

pub struct ScanOptions {
    /// list is in reverse order
    pub is_rev: bool,
    /// scan operation
    pub op: ScanOperation,
    /// The prefix that bounds the scan. All returned keys must start with this prefix.
    /// For point ops (Eq/Ne/In) this is the field-level prefix
    /// (e.g., "tasks-state-"), and `key` is the full value prefix passed to
    /// [`KvStore::scan_prefix`].
    /// For [`ScanOperation::Range`] the prefix is the same field-level prefix;
    /// the interval bounds are full keys and already contain the prefix.
    pub prefix: String,
}

impl ScanOptions {
    pub fn new(op: ScanOperation, prefix: String, is_rev: bool) -> Self {
        Self { is_rev, op, prefix }
    }
}

pub enum ScanOperation {
    /// Not equal — keys that start with the parent prefix but NOT with `key`
    Ne,

    /// Equal — keys that start with `key` (a full value-key prefix)
    Eq,

    /// Key starts with any one of the given full value-key prefixes (e.g.,
    /// "tasks-state-Completed-").
    In { values: Vec<String> },

    /// Half-open interval over full keys inside the scan prefix: every
    /// returned key satisfies `lower <= key` (when `lower` is `Some`) and
    /// `key < upper` (when `upper` is `Some`).
    ///
    /// Bounds are computed by the collection layer, never by backends:
    /// inclusive value-range ends are encoded with the sentinel
    /// [`crate::utils::consts::KEY_SEP_SUCC`], so a bound `..<v>-` group is
    /// addressed as `upper = ..<v>+SUCC` (covers every `..<v>-<id>` key and
    /// nothing above it). Backends only compare full key strings.
    Range {
        lower: Option<String>,
        upper: Option<String>,
    },
}

/// One mutation of an atomic [`KvStore::batch`] write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreBatchOp {
    /// Store (overwrite) `value` under `key`.
    Put { key: String, value: Vec<u8> },
    /// Remove `key`.
    Delete { key: String },
}

#[async_trait::async_trait]
pub trait KvStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;

    /// Apply every mutation of `ops` as one unit: on success all ops are
    /// visible; on failure — for backends that commit atomically — none is.
    /// The collection layer uses this for `create`/`update`/`delete` so a
    /// document row and its index entries are written together instead of as
    /// a sequence of independently committed keys that a mid-write failure
    /// could tear.
    ///
    /// `MemoryStore`, `SledStore`, `SqliteStore`, `PostgresStore` and
    /// `RedisStore` commit through a native transaction/batch (a single-op
    /// batch falls back to `put`/`delete`, skipping transaction overhead).
    /// A backend without cross-key transactions (`NatsStore` — JetStream KV
    /// is per-key) inherits the default sequential loop: order is preserved,
    /// but a mid-batch failure leaves the earlier ops applied.
    async fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } => self.put(key, value.clone()).await?,
                StoreBatchOp::Delete { key } => self.delete(key).await?,
            }
        }
        Ok(())
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>>;
}

#[async_trait::async_trait]
pub trait DbCollection: Send + Sync {
    type Item;
    async fn exists(&self, id: &str) -> Result<bool>;
    async fn find(&self, id: &str) -> Result<Self::Item>;
    async fn query(&self, query: &Query) -> Result<PageData<Self::Item>>;
    async fn create(&self, data: &Self::Item) -> Result<bool>;
    async fn update(&self, data: &Self::Item) -> Result<bool>;
    async fn delete(&self, id: &str) -> Result<bool>;
}
