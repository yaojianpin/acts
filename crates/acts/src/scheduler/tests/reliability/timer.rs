//! Timer reliability: the two store-facing scheduler timers under the
//! conditions the matrix calls out.
//!
//! - the message-retry timer (unacknowledged deliveries are re-sent, retries
//!   exhaust into `Error`) across a store outage and across an engine restart;
//! - the schedule-trigger timer across an engine restart, across a store that
//!   stays down (the loop must degrade instead of hammering it) and across a
//!   duplicate view of the same due fire (a due row seen again must start
//!   exactly one process, never a burst).
//!
//! The wrappers live here on purpose: every helper a case needs is local to
//! this file.

use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use super::*;
use parking_lot::Mutex;
use serial_test::serial;

use crate::{
    ActError, ChannelOptions, Config, Engine, Message, Vars, Workflow,
    config::ConfigData,
    data::DeliveryStatus,
    event::MessageState,
    scheduler::{
        Runtime,
        health::{DEGRADE_AFTER, HealthState},
        runtime::TEST_TICK_MS,
    },
    store::{
        KvStore, MemoryStore, ScanOptions, Store, StoreIden,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::consts::KEY_SEP,
    utils::test::{USES_IRQ, USES_SET, create_proc, create_proc_with_config},
};

/// The workflow under test: an irq act that waits for a response nobody
/// sends, so the process stays running and its unacknowledged deliveries stay
/// open (a finished process's deliveries are closed and swept instead).
fn irq_workflow() -> Workflow {
    Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    })
}

/// A schedule-trigger model whose fired processes stay alive (same irq act),
/// so every fire leaves exactly one more running process — process counting
/// is not perturbed by the settled-process sweeper. The trigger is assigned
/// directly because `with_trigger` takes a non-capturing fn pointer.
fn schedule_model(id: &str, cron: &str) -> Workflow {
    let mut model = Workflow::new().with_id(id).with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    model.on = vec![crate::Trigger {
        id: "tick".to_string(),
        kind: "schedule".to_string(),
        schedule: Some(cron.to_string()),
        ..Default::default()
    }];
    model
}

type Seen = Arc<Mutex<Vec<Message>>>;

/// Register an ack channel that records every workflow-created message but
/// never acknowledges it — the shape the retry timer keeps re-sending.
fn record_without_ack(engine: &Engine) -> Seen {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let rx = seen.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "e1".to_string(),
            ack: true,
            ..Default::default()
        })
        .on_message(move |e| {
            let rx = rx.clone();
            async move {
                if e.r#type == "workflow" && e.state() == MessageState::Created {
                    rx.lock().push(e.inner().clone());
                }
            }
        });
    seen
}

/// The workflow-created delivery row of one process (its message row says
/// `workflow`), so a case can track one concrete retry unit.
async fn workflow_delivery(store: &Store, pid: &str) -> Option<crate::store::data::Delivery> {
    let rows = store
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string()))))
        .await
        .unwrap();
    for row in rows.rows {
        let msg = store.messages().find(&row.msg_id).await.unwrap();
        if msg.r#type == "workflow" {
            return Some(row);
        }
    }
    None
}

async fn proc_count(store: &Store, mid: &str) -> usize {
    store
        .procs()
        .query(
            &Query::new()
                .limit(1000)
                .filter(Filter::and().expr(Expr::eq("mid", mid.to_string()))),
        )
        .await
        .unwrap()
        .count
}

/// Poll with a deadline (the established 20ms-step convention). `ready` runs
/// a store query each step, so it returns a future.
async fn poll_until<F, Fut>(timeout_millis: u64, mut ready: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let steps = (timeout_millis / 20).max(1);
    for _ in 0..steps {
        if ready().await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    ready().await
}

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

/// KV backend whose reads over the `deliveries` collection fail while armed —
/// exactly what the retry timer's scan (`with_no_response_deliveries` starts
/// with `matching_ids`) hits first. Every other key passes through, so the
/// outage is scoped to the collection the timer is about.
struct GatedKv {
    inner: Arc<MemoryStore>,
    deliveries_down: AtomicBool,
}

impl GatedKv {
    fn deliveries_down(&self, down: bool) {
        self.deliveries_down.store(down, Ordering::SeqCst);
    }

    /// Fail the read while the gate is armed.
    fn read(&self, key: &str) -> crate::Result<()> {
        let prefix = AsRef::<str>::as_ref(&StoreIden::Deliveries);
        if self.deliveries_down.load(Ordering::SeqCst) && key.starts_with(prefix) {
            return Err(ActError::Store(
                "deliveries backend unavailable".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl KvStore for GatedKv {
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

/// RETRY × DB失败: the retry scan hits a store outage. The timer must degrade
/// without panicking and without corrupting the row it can no longer scan
/// (no retry inflation, no spurious `Error`), and once the store answers
/// again the still-unacknowledged delivery is re-armed and re-sent.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_retry_survives_store_outage_and_rearms_on_heal() {
    bounded(
        "sch_message_retry_survives_store_outage_and_rearms_on_heal",
        sch_message_retry_survives_store_outage_and_rearms_on_heal_inner(),
    )
    .await;
}

async fn sch_message_retry_survives_store_outage_and_rearms_on_heal_inner() {
    let inner = Arc::new(MemoryStore::new());
    let kv = Arc::new(GatedKv {
        inner: inner.clone(),
        deliveries_down: AtomicBool::new(false),
    });
    let engine = Engine::builder()
        .set_store(kv.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let store = rt.cache().store();
    // reads that bypass the gate — while the gate is armed the gated store
    // must not be read (the loop treats reads as part of the outage)
    let raw = Arc::new(Store::new(inner.clone()));

    let seen = record_without_ack(&engine);
    let id = utils::longid();
    let proc = rt.create_proc(&id, &irq_workflow());
    rt.launch(&proc).await.unwrap();

    // healthy store: the unacked delivery is re-sent at least once
    assert!(
        poll_until(15_000, || async { seen.lock().len() >= 2 }).await,
        "the retry timer must re-send the unacked delivery while the store is healthy"
    );
    let delivery = workflow_delivery(&store, &id)
        .await
        .expect("workflow delivery row");
    assert!(
        delivery.retry_times >= 1,
        "the row must already have been re-armed"
    );
    assert_eq!(delivery.status, DeliveryStatus::Delivered);

    // outage on exactly the collection the timer scans
    kv.deliveries_down(true);
    let frozen = raw
        .deliveries()
        .find(&delivery.id)
        .await
        .expect("delivery readable through the raw backend");

    // the loop degrades on its own (no panic, no silent idle)
    assert!(
        poll_until(15_000, || async {
            rt.scheduler_health().retry.state == HealthState::Degraded
        })
        .await,
        "the retry loop must report degraded while its store reads fail"
    );

    // a few ticks pass under the outage: the frozen row must not move —
    // no retry inflation, no flipped status, no corruption
    tokio::time::sleep(std::time::Duration::from_millis(TEST_TICK_MS * 3)).await;
    let stalled = raw
        .deliveries()
        .find(&delivery.id)
        .await
        .expect("delivery still readable through the raw backend");
    assert_eq!(
        stalled.retry_times, frozen.retry_times,
        "a store outage must not advance the delivery's retry state"
    );
    assert_eq!(stalled.status, DeliveryStatus::Delivered);
    assert_eq!(stalled.update_time, frozen.update_time);
    let seen_at_outage = seen.lock().len();

    // heal: the loop recovers on its own and re-arms the delivery again
    kv.deliveries_down(false);
    assert!(
        poll_until(30_000, || async {
            rt.scheduler_health().retry.state == HealthState::Healthy
        })
        .await,
        "the retry loop must recover once the store answers again"
    );
    assert!(
        poll_until(15_000, || async {
            raw.deliveries()
                .find(&delivery.id)
                .await
                .map(|d| d.retry_times > frozen.retry_times)
                .unwrap_or(false)
        })
        .await,
        "the healed timer must re-arm the still-unacked delivery"
    );
    assert!(
        poll_until(15_000, || async { seen.lock().len() > seen_at_outage }).await,
        "the healed timer must re-send the delivery to its channel"
    );

    // the row is still the same unit: one message, one delivery, no duplicates
    let healed = raw.deliveries().find(&delivery.id).await.unwrap();
    assert_eq!(healed.msg_id, delivery.msg_id);
    assert_eq!(healed.chan_id, delivery.chan_id);
    assert_eq!(healed.status, DeliveryStatus::Delivered);
    engine.close().await;
}

/// RETRY × 重启: a delivery that exhausted its retries before the restart
/// stays `Error` (no resurrection, no new delivery of its message), while an
/// unacknowledged live delivery is picked up again by the restarted engine's
/// retry timer and re-sent to the re-registered channel.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_retry_resumes_after_restart_and_errored_stays_dead() {
    bounded(
        "sch_message_retry_resumes_after_restart_and_errored_stays_dead",
        sch_message_retry_resumes_after_restart_and_errored_stays_dead_inner(),
    )
    .await;
}

async fn sch_message_retry_resumes_after_restart_and_errored_stays_dead_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let config = Config {
        data: ConfigData {
            max_message_retry_times: Some(2),
            ..ConfigData::default()
        },
        ..Default::default()
    };

    // phase 1: exhaust one delivery, keep a second one open
    let engine = Engine::builder()
        .set_store(store.clone())
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let store1 = rt.cache().store();
    let _seen1 = record_without_ack(&engine);

    let pid1 = utils::longid();
    let proc1 = rt.create_proc(&pid1, &irq_workflow());
    rt.launch(&proc1).await.unwrap();

    // the unacked delivery exhausts its retries and turns Error
    let mut errored = None;
    for _ in 0..300 {
        let rows = store1
            .deliveries()
            .query(
                &Query::new().filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid1.to_string()))
                        .expr(Expr::eq("status", DeliveryStatus::Error as i8)),
                ),
            )
            .await
            .unwrap();
        if let Some(row) = rows.rows.first() {
            errored = Some(row.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let errored = errored.expect("a delivery must turn Error before the restart");
    assert_eq!(
        errored.retry_times,
        config.max_message_retry_times(),
        "the row must have exhausted exactly its retry budget"
    );
    let errored_msg = store1.messages().find(&errored.msg_id).await.unwrap();
    let errored_delivery_count = store1
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("msg_id", errored.msg_id.clone()))))
        .await
        .unwrap()
        .count;

    // a second process whose deliveries are still open when the engine dies
    let pid2 = utils::longid();
    let proc2 = rt.create_proc(&pid2, &irq_workflow());
    rt.launch(&proc2).await.unwrap();
    let mut live = None;
    for _ in 0..300 {
        if let Some(row) = workflow_delivery(&store1, &pid2).await {
            live = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let live = live.expect("the live process must have a workflow delivery row");
    assert!(
        matches!(
            live.status,
            DeliveryStatus::Created | DeliveryStatus::Delivered
        ),
        "the live delivery must still be open before the restart"
    );
    let live_delivery_count = store1
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("msg_id", live.msg_id.clone()))))
        .await
        .unwrap()
        .count;

    engine.close().await;

    // phase 2: the restarted engine resumes redelivery on the shared store
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let store2 = engine2.runtime().cache().store();
    let seen2 = record_without_ack(&engine2);

    // the live delivery is re-armed again — by engine2, its retry budget
    // grows past the pre-restart value...
    assert!(
        poll_until(15_000, || async {
            store2
                .deliveries()
                .find(&live.id)
                .await
                .map(|row| row.retry_times > live.retry_times)
                .unwrap_or(false)
        })
        .await,
        "the restarted engine must resume re-arming the open delivery"
    );
    // ...and the row is re-sent to the re-registered channel (only engine2
    // can emit here — the first engine is closed)
    assert!(
        poll_until(15_000, || async {
            seen2
                .lock()
                .iter()
                .any(|m| m.delivery_id.as_deref() == Some(live.id.as_str()) && m.pid == pid2)
        })
        .await,
        "the restarted engine must re-send the open delivery to its channel"
    );

    // the errored delivery stays dead: same status, same retry state, no new
    // delivery of its message, and nothing of its process re-delivered
    let dead = store2.deliveries().find(&errored.id).await.unwrap();
    assert_eq!(
        dead.status,
        DeliveryStatus::Error,
        "an errored delivery must not be resurrected by the restart"
    );
    assert_eq!(dead.retry_times, errored.retry_times);
    assert_eq!(dead.update_time, errored.update_time);
    let dead_deliveries = store2
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("msg_id", errored.msg_id.clone()))))
        .await
        .unwrap();
    assert_eq!(
        dead_deliveries.count, errored_delivery_count,
        "the restart must not create another delivery of the errored message"
    );
    // the errored delivery's canonical message row survives the restart —
    // dead, but intact for a manual resend
    let dead_msg = store2.messages().find(&errored.msg_id).await.unwrap();
    assert_eq!(dead_msg.id, errored_msg.id);
    assert_eq!(dead_msg.pid, errored_msg.pid);
    assert!(
        !seen2.lock().iter().any(|m| m.pid == pid1),
        "no message of the errored process may be re-delivered after the restart"
    );

    // redelivery reuses the one stored delivery row: the re-sent message
    // carries the original delivery id and no second row appeared for it
    let redelivered = seen2
        .lock()
        .iter()
        .find(|m| m.delivery_id.as_deref() == Some(live.id.as_str()))
        .cloned()
        .expect("redelivery observed above");
    assert_eq!(redelivered.id, live.msg_id);
    assert_eq!(redelivered.pid, pid2);
    let live_rows = store2
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("msg_id", live.msg_id.clone()))))
        .await
        .unwrap();
    assert_eq!(
        live_rows.count, live_delivery_count,
        "redelivery must reuse the stored delivery row, not create new ones"
    );

    engine2.close().await;
}

/// SCHEDULE × 重启: a deployed schedule trigger keeps firing after the engine
/// is replaced over the same store — the same single trigger row advances
/// (last_run/next_run), new processes appear, and the restart must not
/// duplicate the trigger row (which would double-fire every tick).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_trigger_fires_after_engine_restart() {
    bounded(
        "sch_schedule_trigger_fires_after_engine_restart",
        sch_schedule_trigger_fires_after_engine_restart_inner(),
    )
    .await;
}

async fn sch_schedule_trigger_fires_after_engine_restart_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let model_id = "sch-timer-restart";
    let trigger_id = format!("{model_id}:tick");

    // phase 1: deploy an every-second schedule and wait for a fire
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let manager = engine.executor(&crate::Principal::unrestricted());
    manager
        .model()
        .deploy(&schedule_model(model_id, "* * * * * *"), None)
        .await
        .unwrap();
    let store1 = engine.runtime().cache().store();

    let mut last_run_1 = 0;
    for _ in 0..300 {
        let evt = store1.events().find(&trigger_id).await.unwrap();
        if evt.last_run > 0 {
            last_run_1 = evt.last_run;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        last_run_1 > 0,
        "the schedule trigger must fire on the first engine"
    );

    // fired processes stay alive (irq), so the count only grows
    let mut n1 = 0;
    for _ in 0..300 {
        n1 = proc_count(&store1, model_id).await;
        if n1 > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(n1 > 0, "the fire must have started a process");

    engine.close().await;

    // phase 2: the restarted engine keeps the trigger firing
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let store2 = engine2.runtime().cache().store();

    let mut evt = None;
    for _ in 0..300 {
        let row = store2.events().find(&trigger_id).await.unwrap();
        if row.last_run > last_run_1 {
            evt = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let evt = evt.expect("the schedule trigger must fire again on the restarted engine");
    assert!(
        evt.next_run > evt.last_run,
        "the restarted timer must roll the schedule forward"
    );

    for _ in 0..300 {
        if proc_count(&store2, model_id).await > n1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        proc_count(&store2, model_id).await > n1,
        "the post-restart fire must have started a new process"
    );

    // the trigger row itself was not duplicated by the restart: exactly one
    // schedule row for the model, or every tick would fire it twice
    let rows = store2
        .events()
        .query(
            &Query::new().limit(1000).filter(
                Filter::and()
                    .expr(Expr::eq("kind", "schedule"))
                    .expr(Expr::eq("mid", model_id)),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.count, 1,
        "the restart must not duplicate the schedule trigger row"
    );
    assert_eq!(rows.rows[0].id, trigger_id);

    engine2.close().await;
}

/// SCHEDULE × 重复消息: a due fire seen again (the crash window between
/// starting the process and rolling the row re-surfaces the same due moment)
/// must start exactly one process — one due fire, one process, no burst —
/// while the timer keeps ticking.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_duplicate_due_fires_exactly_one_process() {
    bounded(
        "sch_schedule_duplicate_due_fires_exactly_one_process",
        sch_schedule_duplicate_due_fires_exactly_one_process_inner(),
    )
    .await;
}

async fn sch_schedule_duplicate_due_fires_exactly_one_process_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let store = rt.cache().store();
    let manager = engine.executor(&crate::Principal::unrestricted());
    let model_id = "sch-timer-dup";
    let trigger_id = format!("{model_id}:tick");

    // a daily cron: the row is never naturally due during the test, so every
    // process that appears is accounted for by a due fire below
    manager
        .model()
        .deploy(&schedule_model(model_id, "0 0 3 * * *"), None)
        .await
        .unwrap();

    let evt = store.events().find(&trigger_id).await.unwrap();
    assert!(
        evt.next_run > crate::utils::time::time_millis(),
        "deploy must arm the schedule in the future"
    );

    // a few timer ticks pass: nothing is due, nothing starts
    tokio::time::sleep(std::time::Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        proc_count(&store, model_id).await,
        0,
        "a far-future schedule must not fire"
    );

    // the same due fire surfaces again (crash between start and row roll):
    // rewind the row to due while last_run stays put
    let mut evt = store.events().find(&trigger_id).await.unwrap();
    evt.next_run = crate::utils::time::time_millis() - 1000;
    store.events().update(&evt).await.unwrap();

    for _ in 0..300 {
        if proc_count(&store, model_id).await >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        proc_count(&store, model_id).await >= 1,
        "the due fire must start exactly one process"
    );
    let fired = store.events().find(&trigger_id).await.unwrap();
    assert!(fired.last_run > 0, "the fire must be recorded");
    assert!(
        fired.next_run > crate::utils::time::time_millis(),
        "the fire must roll the row to the next cron time"
    );

    // more ticks pass: the consumed due fire must not repeat itself — the
    // duplicate view started one process, and the loop must settle again
    tokio::time::sleep(std::time::Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        proc_count(&store, model_id).await,
        1,
        "one due fire must start exactly one process, never a burst"
    );
    let settled = store.events().find(&trigger_id).await.unwrap();
    assert_eq!(
        settled.last_run, fired.last_run,
        "no further fire may happen while the row is not due"
    );

    // a second due fire: again exactly one process — the accounting between
    // due fires and processes stays 1:1
    let mut evt = store.events().find(&trigger_id).await.unwrap();
    evt.next_run = crate::utils::time::time_millis() - 1000;
    store.events().update(&evt).await.unwrap();

    for _ in 0..300 {
        if proc_count(&store, model_id).await >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(TEST_TICK_MS * 2)).await;
    assert_eq!(
        proc_count(&store, model_id).await,
        2,
        "each due fire must map to exactly one process"
    );
    let settled = store.events().find(&trigger_id).await.unwrap();
    assert!(
        settled.last_run > fired.last_run,
        "the second due fire must be recorded in its turn"
    );
    assert!(
        settled.next_run > crate::utils::time::time_millis(),
        "the row must be rolled to the future again"
    );

    engine.close().await;
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

/// RETRY × 未确认（裸重试）: an unacknowledged delivery is re-sent with the same
/// delivery id, the canonical message row stays single, and the retry state
/// lives on the delivery row of the owning channel.
/// (Moved verbatim from `scheduler::tests::message`.)
#[tokio::test]
async fn sch_message_re_sent_if_not_ack() {
    bounded(
        "sch_message_re_sent_if_not_ack",
        sch_message_re_sent_if_not_ack_inner(),
    )
    .await;
}

async fn sch_message_re_sent_if_not_ack_inner() {
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id).await;
    let _rt = engine.runtime();
    let sig = engine.signal(Vec::<Message>::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| {
        let tx_close = tx_close.clone();
        async move {
            tx_close.close();
        }
    });
    emitter.on_error(move |_| {
        let tx_close2 = tx_close2.clone();
        async move {
            tx_close2.close();
        }
    });

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    engine.channel_with_options(&options).on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.r#type == "workflow" && e.state() == MessageState::Created {
                // not ack the message
                rx.update(|data| data.push(e.inner().clone()));

                if rx.data().len() > 1 {
                    rx.close();
                }
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret.len() > 1);

    // every redelivery carries the same delivery id of the stored row
    assert_eq!(ret[0].delivery_id, ret[1].delivery_id);
    assert!(ret[0].delivery_id.is_some());

    let m = ret.first().unwrap();
    // canonical message row — stored once per message id
    let message = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&m.id)
        .await
        .unwrap();
    assert_eq!(message.r#type, "workflow");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);

    // the delivery row keeps the retry state of this channel: it was handed
    // over (`Delivered`) and never acked, so it keeps being re-sent
    let delivery = engine
        .runtime()
        .cache()
        .store()
        .deliveries()
        .find(&m.delivery_id.clone().unwrap())
        .await
        .unwrap();
    assert_eq!(delivery.status, DeliveryStatus::Delivered);
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);
    assert!(delivery.retry_times > 0);
}

#[tokio::test]
async fn sch_message_error_if_not_ack_and_exceed_max_reties() {
    bounded(
        "sch_message_error_if_not_ack_and_exceed_max_reties",
        sch_message_error_if_not_ack_and_exceed_max_reties_inner(),
    )
    .await;
}

async fn sch_message_error_if_not_ack_and_exceed_max_reties_inner() {
    // the irq act never completes: the process stays running while the
    // unacked request deliveries exhaust their retries (a finished process's
    // rows are closed and deleted by the sweeper instead)
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));
    let id = utils::longid();

    let (engine, proc) = create_proc_with_config(
        &Config {
            data: ConfigData {
                max_message_retry_times: Some(2),
                ..ConfigData::default()
            },
            ..Default::default()
        },
        &workflow,
        &id,
    )
    .await;
    let config = engine.config();
    let _rt = engine.runtime();
    let sig = engine.signal(Vec::<Message>::default());
    let tx = sig.clone();
    let _rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| {
        let tx_close = tx_close.clone();
        async move {
            tx_close.close();
        }
    });
    emitter.on_error(move |_| {
        let tx_close2 = tx_close2.clone();
        async move {
            tx_close2.close();
        }
    });
    let rx = sig.clone();
    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    let e2 = engine.clone();
    engine.channel_with_options(&options).on_message(move |e| {
        let rx = rx.clone();
        let engine = engine.clone();
        async move {
            if e.r#type == "workflow" && e.state() == MessageState::Created {
                // not ack the message
                rx.update(|data| data.push(e.inner().clone()));
            } else if let Some(delivery_id) = &e.delivery_id {
                // ack the other deliveries of this channel
                engine
                    .executor(&crate::Principal::unrestricted())
                    .msg()
                    .ack(delivery_id)
                    .await
                    .unwrap();
            }
        }
    });
    e2.runtime().launch(&proc).await.unwrap();
    // wait until the unacked request deliveries exhausted their retries and
    // turned Error (the process stays running, so nothing closes them early)
    let mut error_row = None;
    for _ in 0..300 {
        let pending = e2
            .runtime()
            .cache()
            .store()
            .deliveries()
            .query(
                &Query::new().filter(
                    Filter::and()
                        .expr(Expr::eq("pid", id.to_string()))
                        .expr(Expr::eq("status", DeliveryStatus::Error as i8)),
                ),
            )
            .await
            .unwrap();
        if let Some(row) = pending.rows.first() {
            error_row = Some(row.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let delivery = error_row.expect("a delivery must turn Error after max retries");

    // canonical message row of the errored delivery
    let message = e2
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&delivery.msg_id)
        .await
        .unwrap();
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);

    // the delivery row turned into error after max retries
    assert_eq!(delivery.status, DeliveryStatus::Error);
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);
    assert_eq!(delivery.retry_times, config.max_message_retry_times());
}

#[tokio::test]
async fn sch_message_redelivery_goes_to_owning_channel_only() {
    bounded(
        "sch_message_redelivery_goes_to_owning_channel_only",
        sch_message_redelivery_goes_to_owning_channel_only_inner(),
    )
    .await;
}

async fn sch_message_redelivery_goes_to_owning_channel_only_inner() {
    // two ack channels share the same emitted messages; channel a acks its
    // deliveries while channel b does not — the retry timer must re-send only
    // channel b's deliveries, never acked channel a again. The irq act keeps
    // the process running (a finished process's open deliveries are closed
    // and deleted by the sweeper instead of re-sent)
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id).await;
    let _rt = engine.runtime();

    let sig_b = engine.signal(Vec::<Message>::default());
    let b_send = sig_b.clone();
    let b_close = sig_b.clone();
    let sig_a = engine.signal(Vec::<Message>::default());
    let a_send = sig_a.clone();
    let a_recv = sig_a.clone();

    let engine_a = engine.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "chan_a".to_string(),
            ack: true,
            ..Default::default()
        })
        .on_message(move |e| {
            let engine_a = engine_a.clone();
            let a_send = a_send.clone();
            async move {
                if let Some(delivery_id) = &e.delivery_id {
                    engine_a
                        .executor(&crate::Principal::unrestricted())
                        .msg()
                        .ack(delivery_id)
                        .await
                        .unwrap();
                }
                if e.r#type == "workflow" && e.state() == MessageState::Created {
                    a_send.update(|data| data.push(e.inner().clone()));
                }
            }
        });

    // channel b: never acks, records the workflow-created redeliveries
    let b_close2 = b_close.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "chan_b".to_string(),
            ack: true,
            ..Default::default()
        })
        .on_message(move |e| {
            let b_send = b_send.clone();
            let b_close2 = b_close2.clone();
            async move {
                if e.r#type == "workflow" && e.state() == MessageState::Created {
                    b_send.update(|data| data.push(e.inner().clone()));
                    if b_close2.data().len() > 1 {
                        b_close2.close();
                    }
                }
            }
        });

    engine.runtime().launch(&proc).await.unwrap();
    let received_b = sig_b.timeout(6000).await;
    assert!(
        received_b.len() > 1,
        "channel b should receive the workflow-created message again, got {:?}",
        received_b.len()
    );

    // both redeliveries are the same delivery of the same message
    assert_eq!(received_b[0].id, received_b[1].id);
    assert_eq!(received_b[0].delivery_id, received_b[1].delivery_id);
    let msg_id = received_b[0].id.clone();

    // channel a saw the message exactly once (its ack stopped the retries)
    let received_a = a_recv.timeout(200).await;
    assert_eq!(received_a.len(), 1, "channel a must not be redelivered");
    assert_eq!(received_a[0].id, msg_id);
}

/// A delivery row can reach the store after the close that settles its task:
/// the emission path and the task's terminal write are concurrent, so a
/// message of a task that is already over can still be stored. Nothing settles
/// a delivery of a finished task afterwards, so such a row is born `Completed`
/// — stored open it would keep the process's rows (and its pid, and its
/// workdir) alive forever, since the sweeper only deletes a process whose
/// deliveries have all settled.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_a_delivery_stored_after_its_task_closed_is_born_settled() {
    bounded(
        "sch_a_delivery_stored_after_its_task_closed_is_born_settled",
        sch_a_delivery_stored_after_its_task_closed_is_born_settled_inner(),
    )
    .await;
}

async fn sch_a_delivery_stored_after_its_task_closed_is_born_settled_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    // s1 finishes on its own, s2 waits for a client: the process — and with it
    // s1's closed task — is still there when the straggler is emitted
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_SET, Vars::new().with("done", 1))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
        });

    // the first message of a finished task names it: by then its task row is
    // closed (the task write is queued before the message is emitted)
    let finished = engine.signal::<(String, String)>((String::new(), String::new()));
    let (f, f2) = finished.double();
    engine.channel().on_message(move |e| {
        let f2 = f2.clone();
        async move {
            if e.is_state(MessageState::Completed) {
                f2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                f2.close();
            }
        }
    });

    let chan = engine.channel_with_options(&ChannelOptions {
        id: "settle-lane".to_string(),
        ack: true,
        ..Default::default()
    });
    let straggler = utils::longid();
    let (delivered, received) = engine.signal::<String>(String::new()).double();
    let wanted = straggler.clone();
    chan.on_message(move |e| {
        let delivered = delivered.clone();
        let wanted = wanted.clone();
        async move {
            if e.id == wanted {
                delivered.update(|d| *d = e.delivery_id.clone().unwrap_or_default());
                delivered.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, tid) = f.recv().await;

    // a message of the closed task, emitted while the process still runs
    rt.emitter().emit_message(&crate::Message {
        id: straggler,
        pid,
        tid,
        ..Default::default()
    });
    let delivery_id = received.recv().await;
    assert!(
        !delivery_id.is_empty(),
        "the straggler of an ack channel must be stored as a delivery"
    );
    let delivery = rt
        .cache()
        .store()
        .deliveries()
        .find(&delivery_id)
        .await
        .unwrap();
    assert_eq!(
        delivery.status,
        crate::data::DeliveryStatus::Completed,
        "a delivery stored after its task closed must be born settled"
    );

    engine.close().await;
}
