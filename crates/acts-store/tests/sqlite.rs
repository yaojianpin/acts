#![cfg(feature = "sqlite")]

use acts::{KvStore, ScanOperation, ScanOptions, Store};
use acts_store::SqliteStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Arc::new(Store::new(Arc::new(
        SqliteStore::open_in_memory().await.unwrap(),
    )))
});

#[tokio::test]
async fn sqlite_scan_pushes_eq_value_prefix_into_where() {
    let store = SqliteStore::open_in_memory().await.unwrap();
    for key in [
        "tasks-state-Ready%-1",
        "tasks-state-ReadyX-1",
        "tasks-state-Ready_1",
        "tasks-state-Readyx-1",
        "tasks-state-Done-1",
    ] {
        store.put(key, vec![]).await.unwrap();
    }

    let rows = store
        .scan_prefix(
            "tasks-state-Ready%",
            ScanOptions::new(ScanOperation::Eq, "tasks-state-".to_string(), false),
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "tasks-state-Ready%-1");
}

#[tokio::test]
async fn sqlite_like_is_case_sensitive_for_binary_keys() {
    let store = SqliteStore::open_in_memory().await.unwrap();
    store.put("tasks-state-ReadyA-1", vec![]).await.unwrap();
    store.put("tasks-state-Readya-1", vec![]).await.unwrap();

    let rows = store
        .scan_prefix(
            "tasks-state-ReadyA-",
            ScanOptions::new(ScanOperation::Eq, "tasks-state-".to_string(), false),
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "tasks-state-ReadyA-1");
}

#[tokio::test]
async fn sqlite_scan_applies_range_bounds() {
    let store = SqliteStore::open_in_memory().await.unwrap();
    for key in ["tasks-n-1", "tasks-n-2", "tasks-n-3", "tasks-n-4"] {
        store.put(key, vec![]).await.unwrap();
    }

    let rows = store
        .scan_prefix(
            "tasks-n-",
            ScanOptions::new(
                ScanOperation::Range {
                    lower: Some("tasks-n-2".to_string()),
                    upper: Some("tasks-n-4".to_string()),
                },
                "tasks-n-".to_string(),
                false,
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        rows.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
        ["tasks-n-2", "tasks-n-3"]
    );
}
