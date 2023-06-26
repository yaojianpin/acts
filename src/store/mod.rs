mod data;
mod db;
mod query;
mod store;

#[cfg(test)]
mod tests;

pub use data::*;
pub use query::{DbKey, Query};
pub use store::{Store, StoreKind};

use crate::ActResult;
use std::sync::Arc;

pub trait DbSet: Send + Sync {
    type Item;
    fn exists(&self, id: &str) -> ActResult<bool>;
    fn find(&self, id: &str) -> ActResult<Self::Item>;
    fn query(&self, query: &Query) -> ActResult<Vec<Self::Item>>;
    fn create(&self, data: &Self::Item) -> ActResult<bool>;
    fn update(&self, data: &Self::Item) -> ActResult<bool>;
    fn delete(&self, id: &str) -> ActResult<bool>;
}

/// Store adapter trait
/// Used to implement custom storage
///
/// # Example
/// ```no_run
/// use acts::{store::{Model, Act, Proc, Task, DbSet}, StoreAdapter};
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
///     fn acts(&self) -> Arc<dyn DbSet<Item =Act>> {
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
    fn acts(&self) -> Arc<dyn DbSet<Item = Act>>;

    fn flush(&self);
}
