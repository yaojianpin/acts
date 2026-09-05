use crate::store::{MemoryStore, Store};
use std::sync::Arc;

crate::gen_store_tests!(async { Arc::new(Store::new(Arc::new(MemoryStore::new()))) });
