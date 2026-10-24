#![cfg(feature = "postgres")]

use acts::{KvStore, ScanOperation, ScanOptions, Store, StoreBatchOp, StoreGuard};
use acts_store::PostgresStore;
use std::sync::Arc;

#[macro_use]
mod common;

gen_store_tests!(async {
    Some(Arc::new(Store::new(Arc::new(
        PostgresStore::open("postgres://postgres:yao@localhost:5433/tests")
            .await
            .unwrap(),
    ))))
});

gen_guarded_batch_tests!({
    // Two pools on one database: the guarded batch locks the guard row (or
    // takes the key's advisory lock for an absent one) inside the transaction
    // that writes the ops — the race below crosses two connections, which is
    // what a second process would open.
    let url = "postgres://postgres:yao@localhost:5433/tests";
    let first: Arc<dyn KvStore> = Arc::new(PostgresStore::open(url).await.unwrap());
    let second: Arc<dyn KvStore> = Arc::new(PostgresStore::open(url).await.unwrap());
    (first, second)
});

gen_kv_tests!(async {
    let store: Arc<dyn KvStore> = Arc::new(
        PostgresStore::open("postgres://postgres:yao@localhost:5433/tests")
            .await
            .unwrap(),
    );
    store
});

#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn postgres_open_reports_connection_failure() {
    // Nothing listens on port 1: the pool reports the failed connection as a
    // store error instead of panicking.
    let error = PostgresStore::open("postgres://postgres:yao@127.0.0.1:1/tests").await;
    assert!(error.is_err());
}

#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn postgres_scan_pushes_eq_value_prefix_into_where() {
    let store = PostgresStore::open("postgres://postgres:yao@localhost:5433/tests")
        .await
        .unwrap();
    let prefix = format!("scan-eq-{}-tasks-state-", shortid());
    for key in [
        format!("{prefix}Ready%-1"),
        format!("{prefix}ReadyX-1"),
        format!("{prefix}Ready_1"),
        format!("{prefix}Done-1"),
    ] {
        store.put(&key, vec![]).await.unwrap();
    }

    let rows = store
        .scan_prefix(
            &format!("{prefix}Ready%"),
            ScanOptions::new(ScanOperation::Eq, prefix.clone(), false),
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, format!("{prefix}Ready%-1"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn postgres_scan_applies_range_bounds() {
    let store = PostgresStore::open("postgres://postgres:yao@localhost:5433/tests")
        .await
        .unwrap();
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

/// A NUL byte is not valid in a Postgres `TEXT` value, so every call that
/// binds it fails in the database. Each failure has to surface as an
/// `ActError::Store` instead of panicking, and a guarded batch must roll back.
#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn postgres_maps_invalid_binding_errors() {
    let url = "postgres://postgres:yao@localhost:5433/tests";
    let store = PostgresStore::open(url).await.unwrap();
    let bad = "pg-bad-\0-key";
    let good = format!("pg-bad-{}-", shortid());

    assert!(store.one(bad).await.is_err());
    assert!(store.put(bad, b"v".to_vec()).await.is_err());
    assert!(store.delete(bad).await.is_err());
    assert!(store.many(&[bad.to_string()]).await.is_err());
    assert!(
        store
            .scan_prefix(
                bad,
                ScanOptions::new(
                    ScanOperation::Range {
                        lower: None,
                        upper: None,
                    },
                    bad.to_string(),
                    false,
                ),
            )
            .await
            .is_err()
    );

    store.put(&good, b"held".to_vec()).await.unwrap();

    // A guard read that cannot bind fails before anything is written.
    assert!(
        store
            .batch(&[], &[StoreGuard::holds(bad, b"x".to_vec())])
            .await
            .is_err()
    );
    // The absent-guard path takes an advisory lock on the key, whose bind
    // fails for the same reason.
    assert!(
        store
            .batch(
                &[StoreBatchOp::Put {
                    key: format!("{good}side"),
                    value: b"s".to_vec(),
                }],
                &[StoreGuard::absent(bad)],
            )
            .await
            .is_err()
    );
    // With a valid guard, the failing op rolls the whole transaction back.
    assert!(
        store
            .batch(
                &[StoreBatchOp::Put {
                    key: bad.to_string(),
                    value: b"x".to_vec(),
                }],
                &[StoreGuard::holds(&good, b"held".to_vec())],
            )
            .await
            .is_err()
    );
    assert_eq!(store.one(&good).await.unwrap(), Some(b"held".to_vec()));
    assert_eq!(store.one(&format!("{good}side")).await.unwrap(), None);

    store.delete(&good).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn postgres_open_reports_table_setup_failure() {
    // The connection succeeds, but the schema search path names no existing
    // schema, so `CREATE TABLE IF NOT EXISTS` fails and `open` returns a
    // store error instead of a half-initialized store.
    let error = PostgresStore::open(
        "postgres://postgres:yao@localhost:5433/tests?options=-csearch_path%3Dacts_missing_schema",
    )
    .await;
    assert!(error.is_err());
}

/// `usage` reports `pg_database_size` for the database the store lives in.
#[tokio::test(flavor = "multi_thread")]
#[serial(store_tests)]
async fn postgres_usage_reports_the_database_size() {
    let store = PostgresStore::open("postgres://postgres:yao@localhost:5433/tests")
        .await
        .unwrap();
    store.put("usage-row", vec![b'x'; 1024]).await.unwrap();

    let usage = store.usage().await.unwrap().unwrap();
    assert_eq!(usage.kind, "postgres");
    assert!(usage.used_bytes.unwrap_or_default() > 0);
}
