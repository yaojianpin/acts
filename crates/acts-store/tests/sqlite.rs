#![cfg(feature = "sqlite")]

use acts::Store;
use acts_store::SqliteStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        SqliteStore::open_in_memory().await.unwrap(),
    )))
});
