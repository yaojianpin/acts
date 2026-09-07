use acts::{MemoryStore, Store};
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async { Arc::new(Store::new(Arc::new(MemoryStore::new()))) });
