#[cfg(feature = "store-sled")]
use std::sync::Arc;
#[cfg(feature = "store-sled")]
use crate::store::{Store, sled::SledStore};

#[cfg(feature = "store-sled")]
crate::gen_store_tests!(Arc::new(
    Store::new(Arc::new(SledStore::open_in_memory().unwrap()))
));
