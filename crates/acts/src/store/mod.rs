pub mod data;
mod collection;
mod kv;
mod memory;
pub mod query;

#[cfg(feature = "store-nats")]
mod nats;
#[cfg(feature = "store-redis")]
mod redis;
#[cfg(feature = "store-sqlite")]
mod sqlite;
#[cfg(feature = "store-postgres")]
mod postgres;

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

pub use kv::KvStore;
#[allow(unused_imports)]
pub use memory::MemoryStore;

#[cfg(feature = "store-nats")]
#[allow(unused_imports)]
pub use nats::NatsStore;
#[cfg(feature = "store-redis")]
#[allow(unused_imports)]
pub use redis::RedisStore;
#[cfg(feature = "store-sqlite")]
#[allow(unused_imports)]
pub use sqlite::SqliteStore;
#[cfg(feature = "store-postgres")]
#[allow(unused_imports)]
pub use postgres::PostgresStore;

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

pub trait DbCollection: Send + Sync {
    type Item;
    fn exists(&self, id: &str) -> Result<bool>;
    fn find(&self, id: &str) -> Result<Self::Item>;
    fn query(&self, query: &Query) -> Result<PageData<Self::Item>>;
    fn create(&self, data: &Self::Item) -> Result<bool>;
    fn update(&self, data: &Self::Item) -> Result<bool>;
    fn delete(&self, id: &str) -> Result<bool>;
}
