use acts::{MemoryStore, Store};
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async { Some(Arc::new(Store::new(Arc::new(MemoryStore::new())))) });

gen_guarded_batch_tests!({
    // One in-memory database, one handle: the guarded batch holds the store's
    // write lock across the guard and the ops, which the two racing writers
    // below contend for.
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    (store.clone(), store)
});
