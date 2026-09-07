#![cfg(feature = "nats")]

use acts::Store;
use acts_store::NatsStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        NatsStore::open("nats://127.0.0.1:4222").await.unwrap(),
    )))
});
