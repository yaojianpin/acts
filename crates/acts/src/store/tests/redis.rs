#[cfg(feature = "store-redis")]
use crate::store::{Store, redis::RedisStore};
#[cfg(feature = "store-redis")]
use std::sync::Arc;

#[cfg(feature = "store-redis")]
crate::gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        RedisStore::open("redis://127.0.0.1:6379").await.unwrap(),
    )))
});
