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
use serde::{Deserialize, Serialize};
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
}

pub trait KvStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
    fn scan_prefix(&self, prefix: &str, is_rev: bool) -> Result<Vec<(String, Vec<u8>)>>;
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
