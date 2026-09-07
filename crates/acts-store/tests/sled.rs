#![cfg(feature = "sled")]

use acts::Store;
use acts_store::SledStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async { Arc::new(Store::new(Arc::new(SledStore::open_in_memory().unwrap()))) });
