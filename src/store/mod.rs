pub mod data;
mod db;
mod query;
mod store;

#[cfg(test)]
mod tests;

use data::*;
pub use query::*;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
pub use store::{Store, StoreKind};

use crate::{ActError, Result};
use std::{error::Error, sync::Arc};

fn map_db_err(err: impl Error) -> ActError {
    ActError::Store(err.to_string())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PageData<T> {
    pub count: usize,
    pub page_num: usize,
    pub page_count: usize,
    pub page_size: usize,
    pub rows: Vec<T>,
}

pub trait DbSet: Send + Sync {
    type Item;
    fn exists(&self, id: &str) -> Result<bool>;
    fn find(&self, id: &str) -> Result<Self::Item>;
    fn query(&self, query: &Query) -> Result<PageData<Self::Item>>;
    fn create(&self, data: &Self::Item) -> Result<bool>;
    fn update(&self, data: &Self::Item) -> Result<bool>;
    fn delete(&self, id: &str) -> Result<bool>;
}

/// Store adapter trait
/// Used to implement custom storage
///
/// # Example
/// ```no_run
/// use acts::{data::{Model, Proc, Task, Package, Message}, DbSet, StoreAdapter};
/// use std::sync::Arc;
/// struct TestStore;
/// impl StoreAdapter for TestStore {
///
///     fn models(&self) -> Arc<dyn DbSet<Item = Model>> {
///         todo!()
///     }
///     fn procs(&self) -> Arc<dyn DbSet<Item =Proc>> {
///         todo!()
///     }
///     fn tasks(&self) -> Arc<dyn DbSet<Item =Task>> {
///         todo!()
///     }
///     fn packages(&self) -> Arc<dyn DbSet<Item =Package>> {
///         todo!()
///     }
///     fn messages(&self) -> Arc<dyn DbSet<Item =Message>> {
///         todo!()
///     }
///     fn init(&self) {}
///     fn close(&self) {}
/// }
/// ```
pub trait StoreAdapter: Send + Sync {
    fn init(&self);

    fn models(&self) -> Arc<dyn DbSet<Item = Model>>;
    fn procs(&self) -> Arc<dyn DbSet<Item = Proc>>;
    fn tasks(&self) -> Arc<dyn DbSet<Item = Task>>;
    fn packages(&self) -> Arc<dyn DbSet<Item = Package>>;
    fn messages(&self) -> Arc<dyn DbSet<Item = Message>>;
    fn close(&self);
}
