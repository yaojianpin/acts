#[cfg(feature = "store-postgres")]
use crate::store::{Store, postgres::PostgresStore};
#[cfg(feature = "store-postgres")]
use std::sync::Arc;

#[cfg(feature = "store-postgres")]
crate::gen_store_tests!(Arc::new(Store::new(Arc::new(
    PostgresStore::open("postgres://postgres:yao@localhost:5433/tests").unwrap()
))));
