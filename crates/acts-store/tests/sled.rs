#![cfg(feature = "sled")]

use acts::{KvStore, ScanOperation, ScanOptions, Store};
use acts_store::SledStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Some(Arc::new(Store::new(Arc::new(
        SledStore::open_in_memory().unwrap(),
    ))))
});

gen_guarded_batch_tests!({
    // Sled holds an exclusive file lock, so one database is one handle here;
    // its transactions still race each other, which is what the two spawns
    // below put under test.
    let store: Arc<dyn KvStore> = Arc::new(SledStore::open_in_memory().unwrap());
    (store.clone(), store)
});

gen_kv_tests!(async {
    let store: Arc<dyn KvStore> = Arc::new(SledStore::open_in_memory().unwrap());
    store
});

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

#[tokio::test]
async fn sled_file_backend_opens_and_operates() {
    let dir = std::env::temp_dir().join(format!(
        "acts-store-sled-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let path = dir.to_string_lossy().into_owned();
    let store = SledStore::open(&path).unwrap();
    store.put("sled-file-key", b"value".to_vec()).await.unwrap();
    assert_eq!(
        store.one("sled-file-key").await.unwrap(),
        Some(b"value".to_vec())
    );
    store.delete("sled-file-key").await.unwrap();
    assert_eq!(store.one("sled-file-key").await.unwrap(), None);
    drop(store);
    // Best effort: the OS may still hold the directory handles on Windows.
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sled_scan_reports_non_utf8_keys() {
    let dir = std::env::temp_dir().join(format!(
        "acts-store-sled-utf8-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let path = dir.to_string_lossy().into_owned();
    // sled accepts arbitrary byte keys; `KvStore` keys are `str`, so a
    // database written by another client can hold rows the scan cannot
    // represent as a Rust string. Insert one through the raw sled handle.
    {
        let raw = sled::open(&path).unwrap();
        raw.insert(b"sled-bad-a\xff", b"v".as_slice()).unwrap();
        raw.flush().unwrap();
    }

    let store = SledStore::open(&path).unwrap();
    let prefix = "sled-bad-";

    // Eq narrows the scan to the value-key prefix, then decodes the row.
    let error = store
        .scan_prefix(
            "sled-bad-a",
            ScanOptions::new(ScanOperation::Eq, prefix.to_string(), false),
        )
        .await;
    assert!(error.is_err());

    // Range starts at the lower bound inside the prefix.
    let error = store
        .scan_prefix(
            prefix,
            ScanOptions::new(
                ScanOperation::Range {
                    lower: Some(prefix.to_string()),
                    upper: None,
                },
                prefix.to_string(),
                false,
            ),
        )
        .await;
    assert!(error.is_err());

    // The generic (Ne) path decodes every row under the prefix.
    let error = store
        .scan_prefix(
            "sled-bad-a",
            ScanOptions::new(ScanOperation::Ne, prefix.to_string(), false),
        )
        .await;
    assert!(error.is_err());

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sled_open_reports_invalid_path() {
    let file = std::env::temp_dir().join(format!(
        "acts-store-sled-bad-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // A regular file cannot be opened as a sled database directory.
    std::fs::write(&file, b"not-a-database").unwrap();
    let error = SledStore::open(file.to_str().unwrap());
    assert!(error.is_err());
    let _ = std::fs::remove_file(&file);
}

#[tokio::test]
async fn sled_scan_range_lower_outside_prefix_uses_prefix_scan() {
    let store = SledStore::open_in_memory().unwrap();
    let prefix = format!(
        "sled-range-{}-",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    for name in ["a", "b"] {
        store.put(&format!("{prefix}{name}"), vec![]).await.unwrap();
    }

    // The lower bound is outside the scan prefix, so the backend cannot start
    // its range there and must fall back to a prefix scan. The bound itself is
    // below every key, so both rows remain in range.
    let rows = store
        .scan_prefix(
            &prefix,
            ScanOptions::new(
                ScanOperation::Range {
                    lower: Some("aaa-outside".to_string()),
                    upper: None,
                },
                prefix.clone(),
                false,
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        rows.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
        [format!("{prefix}a"), format!("{prefix}b")]
    );
}
