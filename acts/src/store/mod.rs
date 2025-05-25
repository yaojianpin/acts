pub mod data;
mod db;
pub mod query;

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
