#![cfg(feature = "sled")]

use acts::{KvStore, ScanOperation, ScanOptions, Store};
use acts_store::SledStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async { Arc::new(Store::new(Arc::new(SledStore::open_in_memory().unwrap()))) });

#[tokio::test]
async fn sled_scan_uses_eq_value_prefix() {
    let store = SledStore::open_in_memory().unwrap();
    for key in [
        "tasks-state-Ready-1",
        "tasks-state-Ready-2",
        "tasks-state-Done-1",
    ] {
        store.put(key, vec![]).await.unwrap();
    }

    let rows = store
        .scan_prefix(
            "tasks-state-Ready-",
            ScanOptions::new(ScanOperation::Eq, "tasks-state-".to_string(), false),
        )
        .await
        .unwrap();

    assert_eq!(
        rows.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
        ["tasks-state-Ready-1", "tasks-state-Ready-2"]
    );
}

#[tokio::test]
async fn sled_scan_applies_range_bounds() {
    let store = SledStore::open_in_memory().unwrap();
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
