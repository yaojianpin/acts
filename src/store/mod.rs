pub mod data;
mod db;
mod query;
mod store;

#[cfg(test)]
mod tests;

use data::*;
pub use query::{Cond, DbKey, Expr, Query};
pub use store::{Store, StoreKind};

use crate::Result;
use std::sync::Arc;

pub trait DbSet: Send + Sync {
    type Item;
    fn exists(&self, id: &str) -> Result<bool>;
    fn find(&self, id: &str) -> Result<Self::Item>;
    fn query(&self, query: &Query) -> Result<Vec<Self::Item>>;
    fn create(&self, data: &Self::Item) -> Result<bool>;
    fn update(&self, data: &Self::Item) -> Result<bool>;
    fn delete(&self, id: &str) -> Result<bool>;
}

/// Store adapter trait
/// Used to implement custom storage
///
/// # Example
/// ```no_run
/// use acts::{data::{Model, Proc, Task}, DbSet, StoreAdapter};
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
///     fn init(&self) {}
///     fn flush(&self) {}
/// }
/// ```
pub trait StoreAdapter: Send + Sync {
    fn init(&self);

    fn models(&self) -> Arc<dyn DbSet<Item = Model>>;
    fn procs(&self) -> Arc<dyn DbSet<Item = Proc>>;
    fn tasks(&self) -> Arc<dyn DbSet<Item = Task>>;

    fn flush(&self);
}
