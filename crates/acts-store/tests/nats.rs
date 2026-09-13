#![cfg(feature = "nats")]

//! Live NATS backend suite.
//!
//! Requires a reachable JetStream server, selected by `ACTS_NATS_URL`
//! (default `nats://127.0.0.1:4222`). When no server answers within a short
//! probe timeout the suite reports a single skip instead of failing every
//! case at setup — keep plain `cargo test --features nats` green without a
//! NATS server. CI runs it against a service container.

use acts::Store;
use acts_store::NatsStore;
use std::sync::Arc;
use std::time::Duration;

#[macro_use]
mod common;

/// Print a single skip notice for the whole test binary when the backend is
/// unreachable, so a missing external service is not reported as one failure
/// per test case.
fn skip_backend(reason: &str) {
    static NOTICE: std::sync::Once = std::sync::Once::new();
    NOTICE.call_once(|| eprintln!("skip: {reason}"));
}

/// Cached reachability probe. `Some(false)` means an earlier test in this
/// binary already found no server, so the remaining tests skip without paying
/// the probe timeout again — one connect probe per binary instead of one per
/// test. A successful probe caches `Some(true)` and each test still opens its
/// own store (the backends bind their connection tasks to the calling
/// runtime).
static REACHABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

gen_store_tests!(async {
    let url =
        std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
    if REACHABLE.get() == Some(&false) {
        return None;
    }
    // `NatsStore::open` retries internally, so an unreachable server surfaces
    // as this timeout rather than an immediate connect error.
    match tokio::time::timeout(Duration::from_secs(2), NatsStore::open(&url)).await {
        Ok(Ok(store)) => {
            let _ = REACHABLE.set(true);
            Some(Arc::new(Store::new(Arc::new(store))))
        }
        Ok(Err(err)) => {
            let _ = REACHABLE.set(false);
            skip_backend(&format!("no NATS server reachable at {url}: {err}"));
            None
        }
        Err(_) => {
            let _ = REACHABLE.set(false);
            skip_backend(&format!(
                "no NATS server reachable at {url}: connect timed out"
            ));
            None
        }
    }
});
