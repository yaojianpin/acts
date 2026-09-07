#![cfg(feature = "postgres")]

use acts::Store;
use acts_store::PostgresStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        PostgresStore::open("postgres://postgres:yao@localhost:5433/tests")
            .await
            .unwrap(),
    )))
});
