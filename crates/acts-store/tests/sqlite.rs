#![cfg(feature = "sqlite")]

use acts::{KvStore, ScanOperation, ScanOptions, Store};
use acts_store::SqliteStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Some(Arc::new(Store::new(Arc::new(
        SqliteStore::open_in_memory().await.unwrap(),
    ))))
});

gen_guarded_batch_tests!({
    // One file, two pools: the guarded batch is committed by two independent
    // connections, which is what a second process would open. The file is
    // keyed by the test process (the keys inside are unique per run) and is
    // left in the temp dir rather than deleted under the open handles.
    let path = std::env::temp_dir().join(format!("acts-store-guard-{}.db", std::process::id()));
    let path = path.to_string_lossy().into_owned();
    let first: Arc<dyn KvStore> = Arc::new(SqliteStore::open(&path).await.unwrap());
    let second: Arc<dyn KvStore> = Arc::new(SqliteStore::open(&path).await.unwrap());
    (first, second)
});

gen_kv_tests!(async {
    let store: Arc<dyn KvStore> = Arc::new(SqliteStore::open_in_memory().await.unwrap());
    store
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

#[tokio::test]
async fn sqlite_file_backend_persists_across_reopen() {
    let path = std::env::temp_dir().join(format!(
        "acts-store-sqlite-file-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let path_str = path.to_string_lossy().into_owned();

    {
        let store = SqliteStore::open(&path_str).await.unwrap();
        store
            .put("sqlite-file-key", b"persisted".to_vec())
            .await
            .unwrap();
    }

    // Reopening runs the table setup against an existing database: the table
    // must already hold the committed row.
    let store = SqliteStore::open(&path_str).await.unwrap();
    assert_eq!(
        store.one("sqlite-file-key").await.unwrap(),
        Some(b"persisted".to_vec())
    );
    store.delete("sqlite-file-key").await.unwrap();
    drop(store);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path_str}-wal"));
    let _ = std::fs::remove_file(format!("{path_str}-shm"));
}

#[tokio::test]
async fn sqlite_open_reports_unusable_path() {
    let parent = std::env::temp_dir().join(format!(
        "acts-store-sqlite-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // SQLite creates the database file, not missing parent directories.
    let path = parent.join("nested").join("acts.db");
    let error = SqliteStore::open(path.to_str().unwrap()).await;
    assert!(error.is_err());
}

/// Replace the table with an incompatible one behind the store's back: every
/// statement then fails deterministically (new pooled connections keep the
/// wrong table because `CREATE TABLE IF NOT EXISTS` leaves it alone). The
/// store must report the failures as `ActError::Store` and roll back a batch's
/// open transaction instead of leaking a partial write.
#[tokio::test]
async fn sqlite_maps_failed_statements() {
    use acts::{StoreBatchOp, StoreGuard};

    let path = std::env::temp_dir().join(format!(
        "acts-store-sqlite-errors-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let path_str = path.to_string_lossy().into_owned();
    let store = SqliteStore::open(&path_str).await.unwrap();
    store.put("sqlite-warm", b"v".to_vec()).await.unwrap();

    let mut raw = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::query("DROP TABLE acts_store")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE acts_store (wrong TEXT)")
        .execute(&mut raw)
        .await
        .unwrap();

    assert!(store.one("sqlite-warm").await.is_err());
    assert!(store.put("sqlite-warm", b"v".to_vec()).await.is_err());
    assert!(store.delete("sqlite-warm").await.is_err());
    assert!(store.many(&["sqlite-warm".to_string()]).await.is_err());
    assert!(
        store
            .scan_prefix(
                "sqlite-",
                ScanOptions::new(
                    ScanOperation::Range {
                        lower: None,
                        upper: None,
                    },
                    "sqlite-".to_string(),
                    false,
                ),
            )
            .await
            .is_err()
    );
    // A multi-op batch opens a transaction and rolls it back on failure.
    assert!(
        store
            .batch(
                &[
                    StoreBatchOp::Put {
                        key: "sqlite-a".to_string(),
                        value: vec![],
                    },
                    StoreBatchOp::Put {
                        key: "sqlite-b".to_string(),
                        value: vec![],
                    },
                ],
                &[],
            )
            .await
            .is_err()
    );
    // A guarded batch fails on the guard read.
    assert!(
        store
            .batch(
                &[StoreBatchOp::Put {
                    key: "sqlite-c".to_string(),
                    value: vec![],
                }],
                &[StoreGuard::absent("sqlite-c")],
            )
            .await
            .is_err()
    );

    drop(raw);
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path_str}-wal"));
    let _ = std::fs::remove_file(format!("{path_str}-shm"));
}

/// `usage` reports the database file's page-count times page-size: after a
/// real write the number is above zero, and the kind names the backend.
#[tokio::test]
async fn sqlite_usage_reports_the_database_size() {
    let path =
        std::env::temp_dir().join(format!("acts-store-usage-sqlite-{}.db", std::process::id()));
    let path_str = path.to_string_lossy().into_owned();
    let store = SqliteStore::open(&path_str).await.unwrap();
    store.put("usage-row", vec![b'x'; 1024]).await.unwrap();

    let usage = store.usage().await.unwrap().unwrap();
    assert_eq!(usage.kind, "sqlite");
    assert!(usage.used_bytes.unwrap_or_default() > 0);

    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path_str}-wal"));
    let _ = std::fs::remove_file(format!("{path_str}-shm"));
}
