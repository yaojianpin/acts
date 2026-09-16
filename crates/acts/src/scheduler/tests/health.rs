//! A store that stays down must degrade the loops that poll it.
//!
//! Both store-facing timers treat one failed tick as survivable: the error is
//! logged and the loop goes on. That is right for a transient fault and wrong
//! for a persistent one — the loop keeps querying a store that cannot answer,
//! logs one error per tick, and in every state it reports looks exactly like an
//! idle scheduler. `LoopGuard` gives each loop a failure streak, a backoff
//! window and a state an owner can read; the cases here pin that state machine,
//! and then that the trigger timer degrades, skips its store queries and
//! recovers on its own once the store answers again.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    ActError, Config,
    config::ConfigData,
    scheduler::{
        Runtime,
        health::{DEGRADE_AFTER, HealthState, LoopGuard, MAX_BACKOFF_TICKS},
        runtime::TEST_TICK_MS,
    },
    store::{KvStore, MemoryStore, ScanOptions, StoreIden},
    utils::consts::KEY_SEP,
};

/// Poll `ready` until it holds or `timeout` runs out, so a case waits for the
/// scheduler's own tick to produce the state instead of assuming how many ticks
/// fit in a fixed sleep.
async fn wait_for(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    ready()
}

/// The guard's contract: nothing changes below the threshold, the streak that
/// crosses it degrades the loop and makes it skip the next tick, the window
/// doubles to a cap and no further, and one clean tick clears all of it.
#[test]
fn guard_degrades_widens_to_a_cap_and_recovers() {
    let guard = LoopGuard::new("test");
    let err = ActError::Store("backend unavailable".to_string());

    // a streak below the threshold is only counted: the loop keeps its cadence
    // and is still reported healthy (one transient failure is not an incident)
    for _ in 0..DEGRADE_AFTER - 1 {
        assert!(guard.attempt(), "a healthy loop attempts every tick");
        guard.failed(&err);
    }
    let health = guard.snapshot();
    assert_eq!(health.state, HealthState::Healthy);
    assert_eq!(health.consecutive_failures, DEGRADE_AFTER - 1);
    assert_eq!(health.backoff_ticks, 0);

    // the failure that crosses the threshold degrades the loop and skips
    // exactly one tick: the first window must not already be the cap, or a
    // store that comes back would not be noticed until the cap had elapsed
    assert!(guard.attempt());
    guard.failed(&err);
    let health = guard.snapshot();
    assert_eq!(health.state, HealthState::Degraded);
    assert_eq!(health.consecutive_failures, DEGRADE_AFTER);
    assert_eq!(health.backoff_ticks, 1);
    assert!(!guard.attempt(), "a degraded loop skips its next tick");
    assert!(guard.attempt(), "and attempts the one after it");

    // the window grows with the streak, up to the cap
    let mut widest = 0;
    for _ in 0..MAX_BACKOFF_TICKS + 2 {
        // sit out the window the previous failure set...
        for _ in 0..guard.snapshot().backoff_ticks {
            assert!(!guard.attempt(), "the loop skips its whole window");
        }
        // ...then attempt and fail again, which is what widens it
        assert!(guard.attempt());
        guard.failed(&err);
        widest = widest.max(guard.snapshot().backoff_ticks);
    }
    assert_eq!(
        widest, MAX_BACKOFF_TICKS,
        "the window must reach the cap, and stop there"
    );

    // one clean tick clears the streak and the window, so the loop is back to
    // attempting every tick
    for _ in 0..guard.snapshot().backoff_ticks {
        assert!(!guard.attempt(), "the loop skips its whole window");
    }
    assert!(guard.attempt());
    guard.recovered();
    let health = guard.snapshot();
    assert_eq!(health.state, HealthState::Healthy);
    assert_eq!(health.consecutive_failures, 0);
    assert_eq!(health.backoff_ticks, 0);
    assert!(
        guard.attempt(),
        "a recovered loop attempts every tick again"
    );
}

/// KV backend whose collection *reads* fail while armed. Only the read path is
/// down: a failing write takes the store writer's path, which is not what these
/// loops are subject to. Reads of the `events` collection are the trigger
/// timer's due query, and are counted so a case can tell how often the loop
/// actually reached the store.
struct DownKv {
    inner: MemoryStore,
    down: AtomicBool,
    event_reads: AtomicUsize,
    /// Key prefix of every row and index entry of the `events` collection —
    /// what a read of that collection starts with.
    events_prefix: String,
}

impl DownKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            down: AtomicBool::new(true),
            event_reads: AtomicUsize::new(0),
            events_prefix: format!("{}{}", AsRef::<str>::as_ref(&StoreIden::Events), KEY_SEP),
        }
    }

    fn event_reads(&self) -> usize {
        self.event_reads.load(Ordering::SeqCst)
    }

    /// Count the read, then fail it if the backend is down.
    fn read(&self, key: &str) -> crate::Result<()> {
        if !key.starts_with(&self.events_prefix) {
            return Ok(());
        }
        self.event_reads.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(ActError::Store("backend unavailable".to_string()));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl KvStore for DownKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.read(key)?;
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.read(key)?;
        self.inner.scan_prefix(key, options).await
    }
}

/// The trigger timer against a store that stays down: the loop must report
/// itself degraded, poll the store less than once per tick while it is, and
/// come back to healthy on its own when the store answers.
#[tokio::test]
async fn trigger_loop_degrades_under_a_down_store_and_recovers() {
    let kv = Arc::new(DownKv::new());
    let config = Config {
        data: ConfigData::default(),
        table: Default::default(),
    };
    let rt = Runtime::new(&config, Some(kv.clone())).unwrap();
    rt.init_trigger_timer();

    // the store is down: the streak grows and the loop reports degraded
    assert!(
        wait_for(Duration::from_secs(15), || {
            rt.scheduler_health().trigger.state == HealthState::Degraded
        })
        .await,
        "a loop whose store keeps failing must report degraded"
    );
    let health = rt.scheduler_health().trigger;
    assert!(health.consecutive_failures >= DEGRADE_AFTER);
    assert!(
        health.backoff_ticks > 0,
        "a degraded loop must be skipping ticks, not polling every one: {health:?}"
    );

    // ...and it really is polling less often than once per tick: the skipped
    // ticks are what keeps a dead store from being queried at the tick rate
    // (and from logging one error per tick) for as long as the outage lasts
    let before = kv.event_reads();
    let start = Instant::now();
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 4)).await;
    let reads = kv.event_reads() - before;
    let ticks = (start.elapsed().as_millis() / TEST_TICK_MS as u128) as usize;
    assert!(reads >= 1, "the loop must keep probing the store");
    assert!(
        reads < ticks,
        "backoff must skip ticks: {reads} store queries in {ticks} ticks"
    );
    assert_eq!(rt.scheduler_health().trigger.state, HealthState::Degraded);

    // the store comes back: one clean tick clears the state, with no restart
    // and no separate recovery path
    kv.down.store(false, Ordering::SeqCst);
    assert!(
        wait_for(Duration::from_secs(30), || {
            rt.scheduler_health().trigger.state == HealthState::Healthy
        })
        .await,
        "the loop must recover on its own once the store answers"
    );
    let health = rt.scheduler_health().trigger;
    assert_eq!(health.consecutive_failures, 0);
    assert_eq!(health.backoff_ticks, 0);
    assert!(
        rt.scheduler_health().retry.consecutive_failures == 0,
        "the retry timer was never started, so it has nothing to report"
    );

    rt.close().await;
}
