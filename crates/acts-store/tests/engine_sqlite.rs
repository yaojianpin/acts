#![cfg(feature = "sqlite")]

//! Engine-behavior tests that require a real (async-driver) backend, moved
//! from `acts`'s own test suite when the external store backends moved into
//! this crate.

use acts::{Engine, Vars, Workflow};
use acts_store::SqliteStore;
use serial_test::serial;
use std::sync::Arc;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_set_store_sqlite() {
    let store = SqliteStore::open(":memory:").await.unwrap();
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .start()
        .await
        .unwrap();

    // deploy + read back through the engine must go through the sqlite store
    let model = Workflow::new().with_id("sqlite_model");
    engine
        .executor()
        .model()
        .deploy(&model, None)
        .await
        .unwrap();
    let m = engine
        .executor()
        .model()
        .get("sqlite_model", "")
        .await
        .unwrap();
    assert_eq!(m.id, "sqlite_model");

    engine.close().await;
}

/// Regression: an engine backed by a real (async-driver) store must work on a
/// current-thread tokio runtime. Before the async store migration every
/// async-context store op went through `tokio::task::block_in_place`, which
/// panics on current-thread runtimes — the retry/schedule timers (and every
/// task transition) hit that on their first tick.
#[serial]
#[tokio::test]
async fn engine_sqlite_runs_on_current_thread_runtime() {
    let store = SqliteStore::open_in_memory().await.unwrap();
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .tick_interval_secs(1)
        .start()
        .await
        .unwrap();

    let model = Workflow::new()
        .with_id("current_thread_model")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses("acts.transform.set", Vars::new().with("a", 1))
        });

    let (done, sig) = engine.signal(bool::default()).double();
    engine
        .executor()
        .model()
        .deploy(&model, None)
        .await
        .unwrap();
    engine.channel().on_complete(move |e| {
        let done = done.clone();
        async move {
            if e.mid == "current_thread_model" {
                done.send(true);
            }
        }
    });
    engine
        .executor()
        .proc()
        .start("current_thread_model", Vars::new())
        .await
        .unwrap();

    // long enough for the retry timer to tick (test builds tick every 800ms)
    let ret = sig.timeout(4000).await;
    assert!(ret, "workflow did not complete on a current-thread runtime");
    engine.close().await;
}
