#[cfg(feature = "store-nats")]
use crate::store::{Store, nats::NatsStore};
#[cfg(feature = "store-nats")]
use std::sync::Arc;

#[cfg(feature = "store-nats")]
crate::gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        NatsStore::open("nats://127.0.0.1:4222").await.unwrap(),
    )))
});
