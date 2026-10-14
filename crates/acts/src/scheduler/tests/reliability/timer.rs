//! TIMER reliability cells: the engine's two store-facing background loops —
//! the message-retry timer (unacknowledged deliveries re-sent until their
//! retries exhaust into `Error`) and the schedule-trigger timer (due
//! `schedule` rows fired and rolled forward) — crossed with a store outage, an
//! engine restart, an exhausted retry budget, and a redelivery's channel
//! scope.
//!
//! Both loops belong to the kernel: their rows are the kernel's projection
//! (`deliveries`/`events`) and the two loops run outside the driver, because
//! they are not part of the hop protocol and must not delay it. A cell
//! therefore reads the loops' effects off the row collections and the engine's
//! own channels — a re-send is an `emit_delivery` to the owning channel, a
//! trigger fire is the `events` due query plus `fire_schedule` — and never
//! through a resident scheduler object the cutover removed. A failing store is
//! reported through [`Runtime::scheduler_health`](crate::scheduler::Runtime::scheduler_health),
//! which is what a cell polls instead of sleeping.
//!
//! Several of these properties are also pinned at the store level
//! (`crates/acts/src/store/store.rs`: `retry_scan_rearms_open_rows_and_errors_the_exhausted`,
//! `a_delivery_is_born_settled_when_its_task_already_closed`). Where they are,
//! the cell here is the engine-level half — the same property reached through a
//! real engine, a real run and a real channel — and its doc says so rather than
//! re-deriving the store's own setup.

use std::time::Duration;

use parking_lot::Mutex;
use serial_test::serial;

use crate::config::ConfigData;
use crate::data::DeliveryStatus;
use crate::scheduler::runtime::TEST_TICK_MS;
use crate::scheduler::shell::health::{DEGRADE_AFTER, HealthState};
use crate::utils::test::USES_SET;
use crate::{ChannelOptions, Message, Store};

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

type Seen = Arc<Mutex<Vec<Message>>>;

/// Register an ack channel that records the irq act's message but never
/// acknowledges it — the shape the retry timer keeps re-sending. The returned
/// handle is the shared record of the deliveries the channel received, so a
/// cell can count re-sends and read the delivery id they carry.
///
/// The channel is scoped to the act (`type = act`, `uses = acts.core.irq`)
/// because a run announces more than its acts: the workflow root emits its own
/// `Created` message too, and a channel that matched it would count two initial
/// deliveries per run and store two delivery rows. Scoped this way `seen` names
/// exactly the act's deliveries: the first is the initial dispatch and every
/// later one is a retry-timer re-send of the very same delivery row.
fn record_without_ack(engine: &Engine, id: &str) -> Seen {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let rx = seen.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: id.to_string(),
            ack: true,
            r#type: "act".to_string(),
            uses: USES_IRQ.to_string(),
            ..Default::default()
        })
        .on_message(move |e| {
            let rx = rx.clone();
            async move {
                if e.is_state(MessageState::Created) {
                    rx.lock().push(e.inner().clone());
                }
            }
        });
    seen
}

/// A process's delivery rows. A one-step irq run on one ack channel has exactly
/// one (its act message); a cell names it to follow the retry timer's unit.
async fn deliveries_of(store: &Store, pid: &str) -> Vec<crate::data::Delivery> {
    store
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string()))))
        .await
        .unwrap()
        .rows
}

/// How many processes of one model are live. Every fired process of the
/// schedule fixtures waits in an irq act, so the count only grows — the
/// settled-process sweeper never reclaims them.
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

/// A schedule-trigger model whose fired processes stay alive (the same irq act
/// that waits for a response nobody sends), so every fire leaves exactly one
/// more running process. The trigger is assigned directly because
/// [`Workflow::with_trigger`] takes a plain `fn` and the cron is the cell's own.
fn schedule_model(id: &str, cron: &str) -> Workflow {
    let mut model = Workflow::new().with_id(id).with_step(|step| {
        step.with_id("s1")
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

/// Deploy a schedule model and answer the `events` row id of its trigger.
async fn deploy_schedule(engine: &Engine, model_id: &str, cron: &str) -> String {
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&schedule_model(model_id, cron), None)
        .await
        .unwrap();
    format!("{model_id}:tick")
}

/// Make a schedule row due in the past without recording a fire — the
/// duplicate view of a due moment the crash window between starting a process
/// and rolling the row leaves behind.
async fn make_row_due(store: &Store, trigger_id: &str) {
    let mut evt = store.events().find(trigger_id).await.unwrap();
    evt.next_run = crate::utils::time::time_millis() - 1000;
    store.events().update(&evt).await.unwrap();
}

// ---------------------------------------------------------------------------
// RETRY × DB失败 — the scan across a store outage
// ---------------------------------------------------------------------------

/// A store outage under the retry timer's scan must degrade the loop rather than
/// swallow the failure or corrupt the delivery it can no longer read. The pass
/// fails at its candidate read, before any write, so the row is untouched and
/// the owning channel is *not* re-sent while the store is down; the loop
/// reports the outage through [`Runtime::scheduler_health`], and once the store
/// answers again the still-unacknowledged delivery is re-armed (`retry_times`
/// grows) and re-sent to its channel.
///
/// The harness outage refuses every read and write, so the row's own state
/// cannot be read back mid-outage; what is provable instead is the observable
/// half — the channel is not re-sent and the row keeps its identity — plus the
/// loop's own health. The store-level scan is pinned by
/// `retry_scan_rearms_open_rows_and_errors_the_exhausted`, which reads a frozen
/// row directly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn message_retry_survives_store_outage_and_rearms_on_heal() {
    bounded(
        "message_retry_survives_store_outage_and_rearms_on_heal",
        message_retry_survives_store_outage_and_rearms_on_heal_inner(),
    )
    .await;
}

async fn message_retry_survives_store_outage_and_rearms_on_heal_inner() {
    let kv = OutageKv::new();
    let engine = engine_on(kv.store()).await;
    let store = engine.runtime().store();
    let seen = record_without_ack(&engine, "e1");

    let pid = start_run(&engine, &one_step_irq("act1")).await;

    // healthy store: the unacked delivery is re-armed and re-sent at least once
    poll_until(
        "the retry timer to re-send the unacked delivery while the store is healthy",
        || async { seen.lock().len() >= 2 },
    )
    .await;
    let delivery = deliveries_of(&store, &pid)
        .await
        .into_iter()
        .next()
        .expect("the run's act delivery row");
    assert!(
        delivery.retry_times >= 1,
        "the row must already have been re-armed"
    );
    assert_eq!(delivery.status, DeliveryStatus::Delivered);
    let frozen = delivery;

    // the store goes away: the loop reports it instead of failing silently
    kv.down();
    let reads_before = kv.reads();
    poll_until(
        "the retry loop to report degraded while its store reads fail",
        || async { engine.runtime().scheduler_health().retry.state == HealthState::Degraded },
    )
    .await;
    assert!(
        kv.reads() > reads_before,
        "the degraded loop must have attempted the store"
    );

    // ticks pass under the outage: the pass cannot read its rows, so nothing is
    // re-sent and the row is not advanced behind the still-open client
    let seen_at_outage = seen.lock().len();
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        seen.lock().len(),
        seen_at_outage,
        "a store outage must not re-send the delivery: the pass cannot reach its row"
    );

    // heal: the loop recovers on its own and re-arms the delivery again
    kv.heal();
    poll_until(
        "the retry loop to recover once the store answers",
        || async { engine.runtime().scheduler_health().retry.state == HealthState::Healthy },
    )
    .await;
    let what = format!("the healed timer to re-arm delivery {}", frozen.id);
    poll_until_dumped(
        &what,
        || async {
            store
                .deliveries()
                .find(&frozen.id)
                .await
                .map(|d| d.retry_times > frozen.retry_times)
                .unwrap_or(false)
        },
        || async { String::new() },
    )
    .await;
    poll_until(
        "the healed timer to re-send the delivery to its channel",
        || async { seen.lock().len() > seen_at_outage },
    )
    .await;

    // the row is still the same unit: one message, one delivery, no duplicates
    let healed = store
        .deliveries()
        .find(&frozen.id)
        .await
        .expect("the delivery row survives the outage");
    assert_eq!(healed.msg_id, frozen.msg_id);
    assert_eq!(healed.chan_id, frozen.chan_id);
    assert_eq!(healed.status, DeliveryStatus::Delivered);
    engine.close().await;
}

// ---------------------------------------------------------------------------
// RETRY × restart and an exhausted budget
// ---------------------------------------------------------------------------

/// With `max_message_retry_times = 2`, a delivery that exhausted its retries
/// before the restart stays `Error` — no resurrection, no new delivery of its
/// message, nothing of its process re-delivered — while a live unacknowledged
/// delivery is picked up by the restarted engine's retry timer and re-sent to
/// the re-registered channel, reusing the one stored delivery row.
///
/// The kernel never trusts anything but the store it boots on, so the fresh
/// engine's retry timer scans the same rows the first engine left: the open row
/// is overdue and re-armed, the `Error` row is skipped (the pass acts only on
/// `Created`/`Delivered`). The store-level half of the error transition is
/// `retry_scan_rearms_open_rows_and_errors_the_exhausted`; this cell is the
/// engine-level half across a restart.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn message_retry_resumes_after_restart_and_errored_stays_dead() {
    bounded(
        "message_retry_resumes_after_restart_and_errored_stays_dead",
        message_retry_resumes_after_restart_and_errored_stays_dead_inner(),
    )
    .await;
}

async fn message_retry_resumes_after_restart_and_errored_stays_dead_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let config = Config {
        data: ConfigData {
            max_message_retry_times: Some(2),
            ..ConfigData::default()
        },
        ..Default::default()
    };

    // phase 1: exhaust one delivery, keep a second one open
    let engine = engine_with(store.clone(), &config).await;
    let store1 = engine.runtime().store();
    let _seen1 = record_without_ack(&engine, "e1");

    let pid1 = start_run(&engine, &one_step_irq("act1")).await;
    let what = format!("the delivery of {pid1} to exhaust its retries into Error");
    poll_until_dumped(
        &what,
        || async {
            deliveries_of(&store1, &pid1)
                .await
                .iter()
                .any(|d| d.status == DeliveryStatus::Error)
        },
        || dump_state(&engine, &pid1),
    )
    .await;
    let errored = deliveries_of(&store1, &pid1)
        .await
        .into_iter()
        .find(|d| d.status == DeliveryStatus::Error)
        .expect("an errored delivery row");
    assert_eq!(
        errored.retry_times,
        config.max_message_retry_times(),
        "the row must have exhausted exactly its retry budget"
    );
    let errored_msg = store1.messages().find(&errored.msg_id).await.unwrap();
    let errored_deliveries = deliveries_of(&store1, &pid1).await.len();

    // a second process whose delivery is still open when the engine dies
    let pid2 = start_run(&engine, &one_step_irq("act1")).await;
    let live = poll_first_delivery(&engine, &store1, &pid2).await;
    assert!(
        matches!(
            live.status,
            DeliveryStatus::Created | DeliveryStatus::Delivered
        ),
        "the live delivery must still be open before the restart, got {}",
        live.status
    );
    let live_deliveries = deliveries_of(&store1, &pid2).await.len();

    // stop the first engine with its rows durable
    flush(&engine).await;
    engine.close().await;

    // phase 2: the restarted engine resumes redelivery on the shared store
    let engine2 = engine_with(store.clone(), &config).await;
    let store2 = engine2.runtime().store();
    let seen2 = record_without_ack(&engine2, "e1");

    // the live delivery is re-armed again — by engine2 — and re-sent to the
    // re-registered channel (only engine2 can emit here)
    let what = format!("the restarted engine to re-arm delivery {}", live.id);
    poll_until_dumped(
        &what,
        || async {
            store2
                .deliveries()
                .find(&live.id)
                .await
                .map(|row| row.retry_times > live.retry_times)
                .unwrap_or(false)
        },
        || dump_state(&engine2, &pid2),
    )
    .await;
    poll_until(
        "the restarted engine to re-send the open delivery to its channel",
        || async {
            seen2
                .lock()
                .iter()
                .any(|m| m.delivery_id.as_deref() == Some(live.id.as_str()) && m.pid == pid2)
        },
    )
    .await;

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
    assert_eq!(
        deliveries_of(&store2, &pid1).await.len(),
        errored_deliveries,
        "the restart must not create another delivery of the errored message"
    );
    let dead_msg = store2.messages().find(&errored.msg_id).await.unwrap();
    assert_eq!(dead_msg.id, errored_msg.id);
    assert_eq!(dead_msg.pid, errored_msg.pid);
    assert!(
        !seen2.lock().iter().any(|m| m.pid == pid1),
        "no message of the errored process may be re-delivered after the restart"
    );

    // redelivery reuses the one stored delivery row: the re-sent message
    // carries the original delivery id and no second row appeared for it
    assert_eq!(
        deliveries_of(&store2, &pid2).await.len(),
        live_deliveries,
        "redelivery must reuse the stored delivery row, not create new ones"
    );
    let redelivered = seen2
        .lock()
        .iter()
        .find(|m| m.delivery_id.as_deref() == Some(live.id.as_str()))
        .cloned()
        .expect("the redelivery observed above");
    assert_eq!(redelivered.id, live.msg_id);
    assert_eq!(redelivered.pid, pid2);

    engine2.close().await;
}

/// Poll the first delivery row of `pid` into existence: the act message's
/// dispatch stores it on the process's own shard, so it appears a hop after the
/// run starts.
async fn poll_first_delivery(engine: &Engine, store: &Store, pid: &str) -> crate::data::Delivery {
    let what = format!("the first delivery row of {pid}");
    poll_until_dumped(
        &what,
        || async { !deliveries_of(store, pid).await.is_empty() },
        || dump_state(engine, pid),
    )
    .await;
    deliveries_of(store, pid)
        .await
        .into_iter()
        .next()
        .expect("the delivery observed above")
}

// ---------------------------------------------------------------------------
// SCHEDULE × restart
// ---------------------------------------------------------------------------

/// A deployed schedule trigger keeps firing after the engine is replaced over
/// the same store: the single trigger row advances (`last_run`/`next_run`), new
/// processes appear, and the restart must not duplicate the trigger row — a
/// second row would double-fire every tick.
///
/// The row is the timer's durable state and the fresh engine re-reads it at
/// boot, so nothing about the trigger lives in the engine that died.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_trigger_fires_after_an_engine_restart() {
    bounded(
        "a_schedule_trigger_fires_after_an_engine_restart",
        a_schedule_trigger_fires_after_an_engine_restart_inner(),
    )
    .await;
}

async fn a_schedule_trigger_fires_after_an_engine_restart_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let model_id = "sch-timer-restart";

    // phase 1: deploy an every-second schedule and wait for a fire
    let engine = engine_on(store.clone()).await;
    let trigger_id = deploy_schedule(&engine, model_id, "* * * * * *").await;
    let store1 = engine.runtime().store();

    let what = format!("the schedule {trigger_id} to fire on the first engine");
    poll_until(&what, || async {
        store1
            .events()
            .find(&trigger_id)
            .await
            .map(|e| e.last_run > 0)
            .unwrap_or(false)
    })
    .await;
    let last_run_1 = store1.events().find(&trigger_id).await.unwrap().last_run;
    let what = format!("the fire to start a process of {model_id}");
    poll_until(&what, || async { proc_count(&store1, model_id).await > 0 }).await;
    let n1 = proc_count(&store1, model_id).await;

    engine.close().await;

    // phase 2: the restarted engine keeps the trigger firing
    let engine2 = engine_on(store.clone()).await;
    let store2 = engine2.runtime().store();

    let what = format!("the schedule {trigger_id} to fire again on the restarted engine");
    poll_until(&what, || async {
        store2
            .events()
            .find(&trigger_id)
            .await
            .map(|e| e.last_run > last_run_1)
            .unwrap_or(false)
    })
    .await;
    let evt = store2.events().find(&trigger_id).await.unwrap();
    assert!(
        evt.next_run > evt.last_run,
        "the restarted timer must roll the schedule forward"
    );
    let what = format!("the restart fire of {model_id} to start a process");
    poll_until(&what, || async { proc_count(&store2, model_id).await > n1 }).await;

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

// ---------------------------------------------------------------------------
// SCHEDULE × duplicate due fire
// ---------------------------------------------------------------------------

/// A due fire seen again — the crash window between starting the process and
/// rolling the row re-surfaces the same due moment — must start exactly one
/// process, never a burst: the kernel's roll-forward is at-least-once, so the
/// *start* may be re-driven, but the process must be unique per due fire.
///
/// The kernel keeps the due moment in the row's `next_run`; rolling it forward
/// inside the fire is what closes the window. A row left due starts exactly one
/// more process each time it is seen due, which is the accounting this cell
/// pins: one due moment, one process.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_due_schedule_fire_starts_exactly_one_process() {
    bounded(
        "a_duplicate_due_schedule_fire_starts_exactly_one_process",
        a_duplicate_due_schedule_fire_starts_exactly_one_process_inner(),
    )
    .await;
}

async fn a_duplicate_due_schedule_fire_starts_exactly_one_process_inner() {
    let engine = engine_on(Arc::new(MemoryStore::new())).await;
    let store = engine.runtime().store();
    let model_id = "sch-timer-dup";

    // a daily cron: the row is never naturally due during the test, so every
    // process that appears is accounted for by a due fire below
    let trigger_id = deploy_schedule(&engine, model_id, "0 0 3 * * *").await;
    let evt = store.events().find(&trigger_id).await.unwrap();
    assert!(
        evt.next_run > crate::utils::time::time_millis(),
        "deploy must arm the schedule in the future"
    );

    // ticks pass: nothing is due, nothing starts
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        proc_count(&store, model_id).await,
        0,
        "a far-future schedule must not fire"
    );

    // the same due moment surfaces again (crash between start and row roll)
    make_row_due(&store, &trigger_id).await;
    let what = format!("the due fire of {model_id} to start a process");
    poll_until(&what, || async { proc_count(&store, model_id).await >= 1 }).await;
    let fired = store.events().find(&trigger_id).await.unwrap();
    assert!(fired.last_run > 0, "the fire must be recorded");
    assert!(
        fired.next_run > crate::utils::time::time_millis(),
        "the fire must roll the row to the next cron time"
    );

    // more ticks pass: the consumed due moment must not repeat itself
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
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

    // a second due fire: again exactly one process, so the accounting between
    // due moments and processes stays 1:1
    make_row_due(&store, &trigger_id).await;
    let what = format!("the second due fire of {model_id} to start a process");
    poll_until(&what, || async { proc_count(&store, model_id).await >= 2 }).await;
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 2)).await;
    assert_eq!(
        proc_count(&store, model_id).await,
        2,
        "each due moment must map to exactly one process"
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

// ---------------------------------------------------------------------------
// SCHEDULE × DB失败 — the trigger loop degrades and recovers
// ---------------------------------------------------------------------------

/// A store that keeps failing the trigger timer's due query must leave the loop
/// reporting itself `Degraded` — from [`DEGRADE_AFTER`] consecutive failed
/// ticks it skips ticks instead of hammering the store — and the first clean
/// tick after the store answers must bring it back to `Healthy` with no restart
/// and no separate recovery path. The recovered loop is live again, which the
/// cell proves by firing a due row.
///
/// A whole-store outage is the harness's seam; the failed tick is the trigger
/// loop's one store round trip, and [`Runtime::scheduler_health`] is where it
/// says so.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn the_trigger_loop_degrades_under_a_down_store_and_recovers() {
    bounded(
        "the_trigger_loop_degrades_under_a_down_store_and_recovers",
        the_trigger_loop_degrades_under_a_down_store_and_recovers_inner(),
    )
    .await;
}

async fn the_trigger_loop_degrades_under_a_down_store_and_recovers_inner() {
    let kv = OutageKv::new();
    let engine = engine_on(kv.store()).await;
    let store = engine.runtime().store();
    let model_id = "sch-timer-down";

    // a far-future schedule: nothing is due until the cell says so, so the only
    // state under observation is the loop's own health
    let trigger_id = deploy_schedule(&engine, model_id, "0 0 3 * * *").await;
    assert_eq!(
        proc_count(&store, model_id).await,
        0,
        "a far-future schedule must not fire"
    );

    // the store goes away: the streak grows and the loop reports degraded,
    // skipping ticks rather than polling at the tick rate
    kv.down();
    let reads_before = kv.reads();
    let what = "the trigger loop to report degraded while its store reads fail";
    poll_until(what, || async {
        let health = engine.runtime().scheduler_health().trigger;
        health.state == HealthState::Degraded && health.backoff_ticks > 0
    })
    .await;
    let health = engine.runtime().scheduler_health().trigger;
    assert!(
        health.consecutive_failures >= DEGRADE_AFTER,
        "a degrading loop must have failed at least {DEGRADE_AFTER} ticks in a row: {health:?}"
    );
    assert!(
        kv.reads() > reads_before,
        "the degraded loop must have attempted the store"
    );

    // the store comes back: one clean tick clears the state
    kv.heal();
    poll_until(
        "the trigger loop to recover once the store answers",
        || async {
            let health = engine.runtime().scheduler_health().trigger;
            health.state == HealthState::Healthy
                && health.consecutive_failures == 0
                && health.backoff_ticks == 0
        },
    )
    .await;

    // ...and the recovered loop is live: it fires a row that is now due
    make_row_due(&store, &trigger_id).await;
    let what = format!("the recovered trigger loop to fire the due row of {model_id}");
    poll_until(&what, || async { proc_count(&store, model_id).await > 0 }).await;
    let fired = store.events().find(&trigger_id).await.unwrap();
    assert!(fired.last_run > 0, "the fire must be recorded");

    engine.close().await;
}

// ---------------------------------------------------------------------------
// RETRY × 未确认 — a bare re-send keeps one delivery
// ---------------------------------------------------------------------------

/// An unacknowledged delivery is re-sent with the same delivery id, the
/// canonical message row stays single, and the retry state lives on the
/// delivery row of the owning channel. This is the engine-level half of
/// `retry_scan_rearms_open_rows_and_errors_the_exhausted`: the same re-arm
/// reached through a real run, a real ack channel and the engine's own
/// redelivery path (`emit_delivery`), so the channel sees the re-send rather
/// than the store test's returned row.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn an_unacked_delivery_is_re_sent_with_its_own_row() {
    bounded(
        "an_unacked_delivery_is_re_sent_with_its_own_row",
        an_unacked_delivery_is_re_sent_with_its_own_row_inner(),
    )
    .await;
}

async fn an_unacked_delivery_is_re_sent_with_its_own_row_inner() {
    let engine = engine_on(Arc::new(MemoryStore::new())).await;
    let store = engine.runtime().store();
    let seen = record_without_ack(&engine, "e1");

    let pid = start_run(&engine, &one_step_irq("act1")).await;
    poll_until(
        "the retry timer to re-send the unacked delivery",
        || async { seen.lock().len() >= 2 },
    )
    .await;

    let seen = seen.lock().clone();
    let first = &seen[0];
    let second = &seen[1];
    assert_eq!(
        first.id, second.id,
        "a redelivery is the same canonical message, not a new one"
    );
    assert_eq!(
        first.delivery_id, second.delivery_id,
        "every redelivery carries the stored delivery id of this channel"
    );
    let delivery_id = first
        .delivery_id
        .clone()
        .expect("an ack channel tags its deliveries with their id");

    // the canonical message row is stored once per message id
    let message = store
        .messages()
        .find(&first.id)
        .await
        .expect("the canonical message row");
    assert_eq!(message.r#type, "act");
    assert_eq!(message.pid, pid);
    assert_eq!(message.state, MessageState::Created);
    let rows = store
        .messages()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone()))))
        .await
        .unwrap();
    assert_eq!(rows.count, 1, "the canonical message row must stay single");

    // the delivery row keeps this channel's retry state: handed over
    // (`Delivered`) and never acked, so it keeps being re-sent
    let delivery = store
        .deliveries()
        .find(&delivery_id)
        .await
        .expect("the delivery row");
    assert_eq!(delivery.status, DeliveryStatus::Delivered);
    assert!(
        delivery.retry_times > 0,
        "an unacked delivery must have been re-armed"
    );
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);

    engine.close().await;
}

// ---------------------------------------------------------------------------
// RETRY × 未确认 — an exhausted budget turns an error and stays dead
// ---------------------------------------------------------------------------

/// An unacknowledged delivery exhausts its retries into `Error` and is never
/// re-sent again: the retry pass counts it up to `max_message_retry_times` and
/// then stops handing it over, while the canonical message row and its process
/// survive for a manual resend. This is the exhausted arm of
/// `retry_scan_rearms_open_rows_and_errors_the_exhausted`, reached through a
/// real engine and a real channel: the *observable* is that the channel stops
/// being re-sent.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn an_unacked_delivery_exhausts_its_retries_into_error() {
    bounded(
        "an_unacked_delivery_exhausts_its_retries_into_error",
        an_unacked_delivery_exhausts_its_retries_into_error_inner(),
    )
    .await;
}

async fn an_unacked_delivery_exhausts_its_retries_into_error_inner() {
    let config = Config {
        data: ConfigData {
            max_message_retry_times: Some(2),
            ..ConfigData::default()
        },
        ..Default::default()
    };
    let engine = engine_with(Arc::new(MemoryStore::new()), &config).await;
    let store = engine.runtime().store();
    let seen = record_without_ack(&engine, "e1");

    let pid = start_run(&engine, &one_step_irq("act1")).await;
    let what = format!("the delivery of {pid} to exhaust its retries into Error");
    poll_until_dumped(
        &what,
        || async {
            deliveries_of(&store, &pid)
                .await
                .iter()
                .any(|d| d.status == DeliveryStatus::Error)
        },
        || dump_state(&engine, &pid),
    )
    .await;
    let delivery = deliveries_of(&store, &pid)
        .await
        .into_iter()
        .find(|d| d.status == DeliveryStatus::Error)
        .expect("the errored delivery row");

    // the row turned error after exactly its retry budget, and the message it
    // carries is intact for a manual resend
    assert_eq!(delivery.status, DeliveryStatus::Error);
    assert_eq!(
        delivery.retry_times,
        config.max_message_retry_times(),
        "the row must have exhausted exactly its retry budget"
    );
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);
    let message = store.messages().find(&delivery.msg_id).await.unwrap();
    assert_eq!(message.pid, pid);
    assert_eq!(message.state, MessageState::Created);

    // an errored delivery is never re-sent again: the channel stops receiving
    let re_sends = seen.lock().len();
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        seen.lock().len(),
        re_sends,
        "an errored delivery must not be re-sent"
    );
    let after = store.deliveries().find(&delivery.id).await.unwrap();
    assert_eq!(after.status, DeliveryStatus::Error);
    assert_eq!(after.retry_times, delivery.retry_times);
    assert_eq!(after.update_time, delivery.update_time);

    engine.close().await;
}

// ---------------------------------------------------------------------------
// RETRY × 未确认 — the re-send reaches its owning channel only
// ---------------------------------------------------------------------------

/// A redelivery reaches its own channel only. Two ack channels share the run's
/// emitted messages, but only one acknowledges: the retry timer re-sends the
/// unacked delivery to *its* owning channel (`emit_delivery(chan_id, …)`) and
/// never to the channel that acked, whose row is `Acked` and skipped by the
/// pass.
///
/// The kernel keeps one row per (message × channel); the re-send names the
/// channel, so a second channel's handler can never receive a delivery that is
/// not its own.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_redelivery_reaches_only_its_owning_channel() {
    bounded(
        "a_redelivery_reaches_only_its_owning_channel",
        a_redelivery_reaches_only_its_owning_channel_inner(),
    )
    .await;
}

async fn a_redelivery_reaches_only_its_owning_channel_inner() {
    let engine = engine_on(Arc::new(MemoryStore::new())).await;

    let seen_a: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen_b: Seen = Arc::new(Mutex::new(Vec::new()));

    // channel a: acknowledges each delivery, so the retry timer stops for it
    let acker = engine.clone();
    let a = seen_a.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "chan_a".to_string(),
            ack: true,
            r#type: "act".to_string(),
            uses: USES_IRQ.to_string(),
            ..Default::default()
        })
        .on_message(move |e| {
            let acker = acker.clone();
            let a = a.clone();
            async move {
                if let Some(id) = &e.delivery_id {
                    acker
                        .executor(&crate::Principal::unrestricted())
                        .msg()
                        .ack(id)
                        .await
                        .unwrap();
                }
                if e.is_state(MessageState::Created) {
                    a.lock().push(e.inner().clone());
                }
            }
        });

    // channel b: never acknowledges, records every workflow-created delivery
    let b = seen_b.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "chan_b".to_string(),
            ack: true,
            r#type: "act".to_string(),
            uses: USES_IRQ.to_string(),
            ..Default::default()
        })
        .on_message(move |e| {
            let b = b.clone();
            async move {
                if e.is_state(MessageState::Created) {
                    b.lock().push(e.inner().clone());
                }
            }
        });

    let pid = start_run(&engine, &one_step_irq("act1")).await;

    poll_until("channel b to receive the message again", || async {
        seen_b.lock().len() >= 2
    })
    .await;
    poll_until("channel a to receive the message", || async {
        !seen_a.lock().is_empty()
    })
    .await;

    // channel a's ack stops its retries: it never sees the message twice,
    // while channel b keeps being re-sent
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        seen_a.lock().len(),
        1,
        "channel a must not be redelivered after its ack"
    );
    let b_seen = seen_b.lock().clone();
    assert!(
        b_seen.len() >= 2,
        "channel b must keep being re-sent, got {}",
        b_seen.len()
    );
    assert_eq!(
        b_seen[0].id, b_seen[1].id,
        "the redelivery is the same canonical message"
    );
    assert_eq!(
        b_seen[0].delivery_id, b_seen[1].delivery_id,
        "and the same owning-channel delivery"
    );
    assert_eq!(
        seen_a.lock()[0].id,
        b_seen[0].id,
        "both channels saw the one emitted message"
    );
    assert_eq!(seen_a.lock()[0].pid, pid);

    engine.close().await;
}

// ---------------------------------------------------------------------------
// A delivery stored after its task closed is born settled
// ---------------------------------------------------------------------------

/// A delivery row can reach the store after the close that settles its task:
/// the emission path and the task's terminal write are concurrent, so a message
/// of a task that is already over can still be stored. Nothing settles a
/// delivery of a finished task afterwards, so such a row is born `Completed` —
/// stored open it would keep the process's rows (and its pid, and its workdir)
/// alive forever, since the sweeper only reclaims a process whose deliveries
/// have all settled.
///
/// This is the engine-level half of
/// `a_delivery_is_born_settled_when_its_task_already_closed`: the store test
/// pins the ordering on the process's own writer shard, and this cell pins that
/// the same outcome is reached through a real run, a real channel and the
/// engine's own emission path — and then, by finishing the run, that the row
/// cannot hold it out of the sweeper.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_stored_after_its_task_closed_is_born_settled() {
    bounded(
        "a_delivery_stored_after_its_task_closed_is_born_settled",
        a_delivery_stored_after_its_task_closed_is_born_settled_inner(),
    )
    .await;
}

async fn a_delivery_stored_after_its_task_closed_is_born_settled_inner() {
    let engine = engine_on(Arc::new(MemoryStore::new())).await;
    let store = engine.runtime().store();

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

    // the first message of a finished task names it (pid + node id)
    let finished = engine.signal::<(String, String)>((String::new(), String::new()));
    let f = finished.clone();
    engine.channel().on_message(move |e| {
        let f = f.clone();
        async move {
            if e.is_state(MessageState::Completed) {
                f.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                f.close();
            }
        }
    });

    // an ack channel scoped to the straggler's own type: only that message
    // stores a delivery row here
    let chan = engine.channel_with_options(&ChannelOptions {
        id: "settle-lane".to_string(),
        ack: true,
        r#type: "straggler".to_string(),
        ..Default::default()
    });
    let straggler = utils::longid();
    let delivered = engine.signal::<String>(String::new());
    let d = delivered.clone();
    let wanted = straggler.clone();
    chan.on_message(move |e| {
        let d = d.clone();
        let wanted = wanted.clone();
        async move {
            if e.id == wanted {
                d.update(|v| *v = e.delivery_id.clone().unwrap_or_default());
                d.close();
            }
        }
    });

    let pid = start_run(&engine, &workflow).await;
    let (named_pid, tid) = finished.recv().await;
    assert_eq!(named_pid, pid, "the message names the run it belongs to");
    assert_eq!(tid, "s1", "the act of a `uses` step is the step's own node");

    // s1's task row is closed before the straggler is emitted
    let what = format!("task {tid} of {pid} to read as closed");
    poll_until_dumped(
        &what,
        || async {
            proc_row(&engine, &pid)
                .await
                .and_then(|proc| proc.task(&tid).map(|t| t.state().is_completed()))
                .unwrap_or(false)
        },
        || dump_state(&engine, &pid),
    )
    .await;

    // a message of the closed task, emitted while the process still runs
    engine.runtime().emitter().emit_message(&crate::Message {
        id: straggler.clone(),
        pid: pid.clone(),
        tid: tid.clone(),
        r#type: "straggler".to_string(),
        ..Default::default()
    });
    let delivery_id = delivered.recv().await;
    assert!(
        !delivery_id.is_empty(),
        "the straggler of an ack channel must be stored as a delivery"
    );
    let delivery = store.deliveries().find(&delivery_id).await.unwrap();
    assert_eq!(
        delivery.status,
        DeliveryStatus::Completed,
        "a delivery stored after its task closed must be born settled"
    );

    // and it cannot hold the run out of the sweeper: finishing s2 lets the
    // whole run — this delivery included — be reclaimed
    wait_waiting_task(&engine, &pid, "s2").await;
    complete_act(&engine, &pid, "s2").await.unwrap();
    wait_completed(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;

    engine.close().await;
}
