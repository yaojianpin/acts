#![cfg(feature = "redis")]

use acts::Store;
use acts_store::RedisStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        RedisStore::open("redis://127.0.0.1:6379").await.unwrap(),
    )))
});
