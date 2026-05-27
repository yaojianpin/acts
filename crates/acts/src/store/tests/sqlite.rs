#[cfg(feature = "store-sqlite")]
use std::sync::Arc;
#[cfg(feature = "store-sqlite")]
use crate::store::{Store, sqlite::SqliteStore};

#[cfg(feature = "store-sqlite")]
crate::gen_store_tests!(Arc::new(
    Store::new(Arc::new(SqliteStore::open_in_memory().unwrap()))
));
