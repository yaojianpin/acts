#![cfg(feature = "rocksdb")]

use acts::{KvStore, ScanOperation, ScanOptions, Store, StoreBatchOp, StoreGuard};
use acts_store::RocksdbStore;
use std::sync::Arc;

#[macro_use]
mod common;

fn in_memory() -> Arc<RocksdbStore> {
    Arc::new(RocksdbStore::open_in_memory().expect("in-memory rocksdb"))
}

gen_store_tests!(async { Some(Arc::new(Store::new(in_memory()))) });

gen_guarded_batch_tests!({
    // RocksDB takes an exclusive lock on one database directory. The two
    // handles share the same instance; its transactions still race each other
    // independently, which is the property the guard needs.
    let store: Arc<dyn KvStore> = in_memory();
    (store.clone(), store)
});

gen_kv_tests!(async {
    let store: Arc<dyn KvStore> = in_memory();
    store
});

#[tokio::test(flavor = "multi_thread")]
async fn rocksdb_scan_applies_eq_and_range_bounds() {
    let store = RocksdbStore::open_in_memory().unwrap();
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

    let rows = store
        .scan_prefix(
            "tasks-n-",
            ScanOptions::new(
                ScanOperation::Range {
                    lower: Some("tasks-n-2".to_string()),
                    upper: Some("tasks-n-5".to_string()),
                },
                "tasks-n-".to_string(),
                true,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
        ["tasks-n-4", "tasks-n-3", "tasks-n-2"]
    );
}

/// A file-backed database survives a close/reopen through its WAL and SSTs.
#[tokio::test(flavor = "multi_thread")]
async fn rocksdb_file_backend_persists_across_reopen() {
    let path = std::env::temp_dir().join(format!(
        "acts-store-rocksdb-{}-{}",
        std::process::id(),
        nanoid::nanoid!()
    ));
    let path_string = path.to_string_lossy().into_owned();
    {
        let store = RocksdbStore::open(&path_string).unwrap();
        store.put("persisted", b"value".to_vec()).await.unwrap();
        store.flush().await.unwrap();
    }
    {
        let store = RocksdbStore::open(&path_string).unwrap();
        assert_eq!(
            store.one("persisted").await.unwrap(),
            Some(b"value".to_vec())
        );
        let usage = store.usage().await.unwrap().unwrap();
        assert_eq!(usage.kind, "rocksdb");
        assert!(usage.used_bytes.unwrap() > 0);
    }
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test(flavor = "multi_thread")]
async fn rocksdb_guarded_batch_rejects_a_stale_guard_without_writing() {
    let store = RocksdbStore::open_in_memory().unwrap();
    store.put("row", b"v1".to_vec()).await.unwrap();
    let applied = store
        .batch(
            &[
                StoreBatchOp::Put {
                    key: "row".to_string(),
                    value: b"v2".to_vec(),
                },
                StoreBatchOp::Put {
                    key: "index".to_string(),
                    value: b"i".to_vec(),
                },
            ],
            &[StoreGuard::holds("row", b"v1".to_vec())],
        )
        .await
        .unwrap();
    assert!(applied);
    let stale = store
        .batch(
            &[StoreBatchOp::Put {
                key: "row".to_string(),
                value: b"stale".to_vec(),
            }],
            &[StoreGuard::holds("row", b"v1".to_vec())],
        )
        .await
        .unwrap();
    assert!(!stale);
    assert_eq!(store.one("row").await.unwrap(), Some(b"v2".to_vec()));
}
