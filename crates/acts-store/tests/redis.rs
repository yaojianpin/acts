#![cfg(feature = "redis")]

use acts::{KvStore, ScanOperation, ScanOptions, Store};
use acts_store::RedisStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        RedisStore::open("redis://127.0.0.1:6379").await.unwrap(),
    )))
});

#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn redis_scan_matches_eq_value_prefix_and_escapes_glob() {
    let store = RedisStore::open("redis://127.0.0.1:6379").await.unwrap();
    let prefix = format!("scan-eq-{}-tasks-state-", shortid());
    for key in [
        format!("{prefix}Ready?-1"),
        format!("{prefix}ReadyX-1"),
        format!("{prefix}Done-1"),
    ] {
        store.put(&key, vec![]).await.unwrap();
    }

    let rows = store
        .scan_prefix(
            &format!("{prefix}Ready?"),
            ScanOptions::new(ScanOperation::Eq, prefix.clone(), false),
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, format!("{prefix}Ready?-1"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn redis_scan_applies_range_bounds() {
    let store = RedisStore::open("redis://127.0.0.1:6379").await.unwrap();
    let prefix = format!("scan-range-{}-tasks-n-", shortid());
    for id in ["1", "2", "3", "4"] {
        store.put(&format!("{prefix}{id}"), vec![]).await.unwrap();
    }

    let rows = store
        .scan_prefix(
            &prefix,
            ScanOptions::new(
                ScanOperation::Range {
                    lower: Some(format!("{prefix}2")),
                    upper: Some(format!("{prefix}4")),
                },
                prefix.clone(),
                false,
            ),
        )
        .await
        .unwrap();

    let mut keys: Vec<_> = rows.into_iter().map(|(key, _)| key).collect();
    keys.sort();
    assert_eq!(keys, [format!("{prefix}2"), format!("{prefix}3")]);
}
