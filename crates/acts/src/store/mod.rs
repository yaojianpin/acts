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
    #[strum(serialize = "events")]
    Events,
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
    /// For point ops (Eq/Gt/Lt/Ge/Le/Ne/Match) this is the field-level prefix
    /// (e.g., "tasks|state|"), and `key` is the full value prefix.
    /// For range ops this equals the `key` parameter.
    pub prefix: String,
}

impl ScanOptions {
    pub fn new(op: ScanOperation, prefix: String, is_rev: bool) -> Self {
        Self { is_rev, op, prefix }
    }
}

pub enum ScanOperation {
    /// Not equal — keys that start with the parent prefix but NOT the given key
    Ne,

    /// Equal — keys that start with the given key
    Eq,

    /// Less than — keys less than the given key
    Lt,

    /// Greater than — keys greater than the given key
    Gt,

    /// Greater and equal — keys greater than or equal to the given key
    Ge,

    /// Less and equal — keys less than or equal to the given key
    Le,

    /// Start with the key (substring match on the value)
    Match,

    /// Key is in the range
    /// The key >= key + SEP + from,
    ///         < key + SEP + to
    Range { from: String, to: String },

    /// Key is in the range
    /// The key > key + SEP + from,
    ///         < key + SEP + to
    ExclusiveRange { from: String, to: String },

    /// Key is in the range
    /// The key >= key + SEP + from,
    ///         <= key + SEP + to
    InclusiveRange { from: String, to: String },

    /// Key starts with any one of the given value keys.
    /// Each entry in `values` is a full value-key prefix (e.g.,
    /// "tasks|state|Completed|").
    In { values: Vec<String> },
}

pub trait KvStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>>;
}

pub trait DbCollection: Send + Sync {
    type Item;
    fn exists(&self, id: &str) -> Result<bool>;
    fn find(&self, id: &str) -> Result<Self::Item>;
    fn query(&self, query: &Query) -> Result<PageData<Self::Item>>;
    fn create(&self, data: &Self::Item) -> Result<bool>;
    fn update(&self, data: &Self::Item) -> Result<bool>;
    fn delete(&self, id: &str) -> Result<bool>;
}
