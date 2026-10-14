//! REMOVE reliability cells: the `Remove` client action under a crash and
//! reload, a store outage, a duplicate delivery, and the sweep of a finished
//! run's own message and delivery rows.
//!
//! The kernel does not serve `Remove`: `Runtime::kernel_kind` answers `None`
//! for the wire event, so `do_action` fails the delivery with
//! `ActError::Action("action Remove is not served by the kernel engine")`
//! *before* the aggregate is read or written. There is no removal to record,
//! to replay, or to degrade, and no per-act durable write of one. The three
//! action cells are therefore stated as the conformance and robustness
//! properties the kernel does hold, plainly and without weakening:
//!
//! - the refusal *is* the delivery's whole effect: it consumes nothing (the
//!   aggregate's version, executions, operations and outbox identical), so a
//!   reload has nothing to replay and the run still settles normally;
//! - the refusal is decided before any store access, so it stays clean while
//!   the store is refused wholesale — and a *served* action's commit is still
//!   what surfaces the outage to the client, after which the healed run
//!   completes exactly once;
//! - a duplicate delivery of the refusal is indistinguishable from the first:
//!   both consume nothing, and the run is still fully drivable.
//!
//! The fourth legacy cell never sent the action either: it pinned a finished
//! run's message and delivery rows being swept with its others. The kernel
//! reaches that same row set through the run's own act messages — the only way
//! the family exists now — so the cell keeps the legacy mechanism (an ack
//! channel whose delivery of the act's `Created` must be reclaimed) and the
//! legacy assertion (the sweep leaves no message and no delivery row).

use std::future::Future;
use std::time::Instant;

use serial_test::serial;

use crate::store::data::DeliveryStatus;
use crate::{ActError, ChannelOptions};

use super::*;

// ---------------------------------------------------------------------------
// Fixtures and the durable decision points
// ---------------------------------------------------------------------------

/// Start one two-step irq run, wait for the head act to wait, and answer
/// `(pid, step1)` with every write of the wait settled — the state a client
/// action is aimed at.
async fn waiting_head(engine: &Engine) -> (String, String) {
    let created = capture_created(engine, "act1");
    let pid = start_run(engine, &two_step_irq_workflow()).await;
    let (announced, step1) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(
        step1, "step1",
        "the act of a `uses` step is the step's own node"
    );
    wait_waiting_task(engine, &pid, &step1).await;
    flush(engine).await;
    (pid, step1)
}

/// Deliver one `Remove` and assert the kernel's answer to it: the delivery is
/// refused as an action the kernel does not serve — never a silent `Ok` (a
/// removal that never happened) and never a store fault (a refusal the client
/// could retry) — and the answer names that action. Answers the refusal.
async fn refused_remove(engine: &Engine, pid: &str, tid: &str) -> String {
    let refusal = match act(engine, pid, tid, EventAction::Remove, Vars::new()).await {
        Err(ActError::Action(text)) => text,
        other => panic!("`Remove` must be refused as an unserved action, got {other:?}"),
    };
    assert!(
        refusal.contains("Remove") && refusal.contains("not served by the kernel engine"),
        "the refusal must name the action the kernel does not serve, got {refusal:?}"
    );
    refusal
}

/// One run's durable decision point, as comparable pieces. A client delivery
/// the kernel refuses must leave every piece identical, so each piece names
/// exactly what would have moved had the delivery been served.
struct Durable {
    /// The aggregate version: a refusal that landed a commit would move it.
    version: String,
    /// The sorted `(node, generation, state)` set: what the run has decided.
    executions: String,
    /// The operation and outbox ids, with their states: what the run has
    /// scheduled and what it has left to deliver.
    scheduled: String,
}

/// The run's durable decision point, read from the aggregate — the kernel's
/// authoritative record (the rows are its projection).
async fn durable_shape(engine: &Engine, pid: &str) -> Durable {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate of a live run exists");
    let mut executions: Vec<String> = snapshot
        .executions
        .iter()
        .map(|(id, execution)| format!("{}#{}={:?}", id.task_id, id.generation, execution.state))
        .collect();
    executions.sort();
    let mut scheduled: Vec<String> = snapshot
        .operations
        .keys()
        .map(|id| format!("op:{id:?}"))
        .collect();
    scheduled.extend(
        snapshot
            .outbox
            .values()
            .map(|record| format!("outbox:{}={:?}", record.id.key, record.state)),
    );
    scheduled.sort();
    Durable {
        version: format!("{:?}", snapshot.version),
        executions: executions.join(","),
        scheduled: scheduled.join(","),
    }
}

/// [`durable_shape`] once the run stops moving: the same shape on three
/// consecutive ticks. The driver's own trailing hop — the delivery
/// confirmation of the act's `Created` — lands just after the client has seen
/// that message, so a decision point has to be read from a settled aggregate;
/// a shape taken mid-hop would measure the driver's own writes instead of the
/// delivery under test.
async fn settled_shape(engine: &Engine, pid: &str) -> Durable {
    let deadline = Instant::now() + WAIT;
    let mut last = durable_shape(engine, pid).await;
    let mut stable = 0;
    loop {
        tokio::time::sleep(TICK).await;
        let now = durable_shape(engine, pid).await;
        if now.version == last.version
            && now.executions == last.executions
            && now.scheduled == last.scheduled
        {
            stable += 1;
            if stable >= 3 {
                return now;
            }
        } else {
            stable = 0;
            last = now;
        }
        assert!(
            Instant::now() < deadline,
            "the run {pid}'s durable shape never settled: {} {} {}",
            last.version,
            last.executions,
            last.scheduled
        );
    }
}

/// The ascending generations the aggregate holds for one model node: how many
/// times the node has been instantiated. Exactly one means the run did that
/// node's work exactly once — a second decision (a replay, a duplicate
/// delivery) would show up here, where a task count cannot see it (the read
/// model keeps one row per model node whichever generation it is).
async fn generations(engine: &Engine, pid: &str, tid: &str) -> Vec<u64> {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate of a live run exists");
    let mut generations: Vec<u64> = snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == tid)
        .map(|id| id.generation)
        .collect();
    generations.sort_unstable();
    generations
}

/// Complete the head act, wait for the act its completion propagates, complete
/// that too, and answer once every execution of the run is terminal — the
/// normal two-step path a refused delivery must leave fully drivable.
///
/// The second act's `Created` is registered *before* the head act completes,
/// so its dispatch cannot be missed.
async fn drive_to_settled(engine: &Engine, pid: &str, step1: &str) {
    let created = capture_created(engine, "act2");
    complete_act(engine, pid, step1).await.unwrap();
    let (announced, step2) = expect_created(&created).await;
    assert_eq!(announced, pid, "the propagated act names its own run");
    assert_eq!(step2, "step2", "the completion propagates the next node");
    wait_waiting_task(engine, pid, &step2).await;
    complete_act(engine, pid, &step2).await.unwrap();
    wait_completed(engine, pid).await;
}

/// Assert a pair of row counts stays zero over `ticks` settle windows: a row
/// deleted with a finished run must not be re-created by a late emission of
/// that run's own message. Each window reads the store on the harness's tick,
/// so a resurrection cannot slip past between two reads.
async fn assert_stays_empty<F, Fut>(what: &str, ticks: usize, mut counts: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = (usize, usize)>,
{
    for _ in 0..ticks {
        let (messages, deliveries) = counts().await;
        assert_eq!(
            (messages, deliveries),
            (0, 0),
            "{what}: a row reappeared after the sweep"
        );
        tokio::time::sleep(SETTLE).await;
    }
}

// ---------------------------------------------------------------------------
// REMOVE × 重启 — the legacy cell replayed a pending remove record; the kernel
// never records one.
// ---------------------------------------------------------------------------

/// The legacy cell pinned a `Remove` whose action record had landed but whose
/// effects had not, replayed exactly once by recovery on reload, and pinned the
/// removal's own durable act row (`Removed`). The kernel does not serve
/// `Remove`: `kernel_kind` answers `None` and the delivery fails with the
/// action error before the aggregate is touched, so nothing about it is ever
/// durable and there is no record for a reload to replay, and the act it would
/// have decided never leaves `Waiting`.
///
/// So the cell pins the conformance half of the legacy intent — the half the
/// kernel holds — and says plainly what replaced the other: the refusal consumes
/// nothing (version, executions, operations and outbox identical) and leaves the
/// run fully drivable, and after a crash and reload over the same store the run
/// still holds its one waiting generation and settles normally through both
/// steps. A reload that found (or invented) a removal would show a changed
/// execution set here — a second generation, or the head act no longer waiting —
/// and the legacy failure mode (a dropped replay leaving the act stuck) cannot
/// arise, because there is nothing to replay or drop.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_remove_consumes_nothing_and_is_not_replayed_on_reload() {
    bounded(
        "a_refused_remove_consumes_nothing_and_is_not_replayed_on_reload",
        a_refused_remove_consumes_nothing_and_is_not_replayed_on_reload_inner(),
    )
    .await;
}

async fn a_refused_remove_consumes_nothing_and_is_not_replayed_on_reload_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_head(&engine).await;
    let before = settled_shape(&engine, &pid).await;

    // the delivery the legacy engine recorded and replayed
    let refusal = refused_remove(&engine, &pid, &step1).await;
    println!("remove refused: {refusal}");
    assert_eq!(
        kv.fired(),
        0,
        "the refusal is decided before the aggregate is touched, so it may not write at all"
    );

    // the refusal consumed nothing: the run's durable shape is exactly what the
    // waiting act left
    let after = settled_shape(&engine, &pid).await;
    assert_eq!(
        after.version, before.version,
        "a refused `Remove` must not land a commit"
    );
    assert_eq!(
        after.executions, before.executions,
        "a refused `Remove` must not decide anything, got {}",
        after.executions
    );
    assert_eq!(
        after.scheduled, before.scheduled,
        "a refused `Remove` must not schedule anything, got {}",
        after.scheduled
    );

    // the crash and the reload: the refusal was never recorded, so the fresh
    // boot has nothing to replay
    let reloaded = restart(&engine, store.clone()).await;
    wait_waiting_task(&reloaded, &pid, &step1).await;
    assert_eq!(
        generations(&reloaded, &pid, &step1).await,
        vec![0],
        "a reload must not re-decide the refused act (no rewind, no second generation)"
    );
    let replayed = settled_shape(&reloaded, &pid).await;
    assert_eq!(
        replayed.executions, before.executions,
        "the reloaded run must hold the decisions the refusal left, got {}",
        replayed.executions
    );

    // still fully drivable to the normal settlement, once
    drive_to_settled(&reloaded, &pid, &step1).await;
    assert_settled(&reloaded, &pid).await;
    assert_eq!(
        generations(&reloaded, &pid, &step1).await,
        vec![0],
        "the head act ran once"
    );
    assert_eq!(
        generations(&reloaded, &pid, "step2").await,
        vec![0],
        "the forward act ran once"
    );
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × DB失败 — the legacy cell degraded a failed removal write and healed;
// the kernel refuses the delivery before the store.
// ---------------------------------------------------------------------------

/// The legacy cell failed the durable write of the removal's state once and
/// pinned the graceful degradation: the fault surfaced at the durability
/// barrier, the act still read `Removed` in memory, and the engine's own
/// follow-up writes re-persisted it exactly once. The kernel does not serve
/// `Remove`, and its refusal is decided *before* any store access: a store that
/// is down cannot turn the refusal into a store fault, and there is no removal
/// state write the outage could land half-way. The nearest property is
/// therefore the robustness one, and it is stronger, not weaker: the refusal
/// consumes nothing under the outage, and the outage is still surfaced to the
/// client by the commit of a *served* action on the same waiting act (the
/// client is handed the store's own failure, which the kernel wraps in its
/// runtime error).
///
/// The healed run is then driven to its normal settlement with one generation
/// per node: the outage left no partial decision for recovery to complete later
/// (which would show as a second generation, or as an act that no longer
/// waits), so the work happens exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_remove_during_an_outage_consumes_nothing_and_still_settles_once() {
    bounded(
        "a_refused_remove_during_an_outage_consumes_nothing_and_still_settles_once",
        a_refused_remove_during_an_outage_consumes_nothing_and_still_settles_once_inner(),
    )
    .await;
}

async fn a_refused_remove_during_an_outage_consumes_nothing_and_still_settles_once_inner() {
    let kv = OutageKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, step1) = waiting_head(&engine).await;
    let before = settled_shape(&engine, &pid).await;

    // the store goes away
    kv.down();

    // the refusal is decided before any store access, so the outage cannot turn
    // it into a store fault — the client gets the action error either way
    let refusal = refused_remove(&engine, &pid, &step1).await;
    println!("remove refused while the store is down: {refusal}");

    // the outage is still surfaced by the engine, on the path that does need
    // the store: a served action's commit is the store's to refuse, and the
    // client is told *that* — the store's own failure, wrapped by the kernel in
    // its runtime error — rather than a silent success or the action refusal
    let attempts_before = kv.reads() + kv.writes();
    let failure = match complete_act(&engine, &pid, &step1).await {
        Err(err) => err.to_string(),
        Ok(()) => panic!("a store that refuses a served action's commit must not report success"),
    };
    assert!(
        failure.contains("store is down"),
        "the refusal must carry the store's own failure to the client, got {failure:?}"
    );
    assert!(
        kv.reads() + kv.writes() > attempts_before,
        "the refused commit must have attempted the store"
    );

    // the outage consumed nothing: the healed run is exactly where the refusal
    // left it, and no parked decision surfaces once the store is back
    kv.heal();
    let after = settled_shape(&engine, &pid).await;
    assert_eq!(
        after.version, before.version,
        "an outage must not move the run's version"
    );
    assert_eq!(
        after.executions, before.executions,
        "an outage must not decide anything, got {}",
        after.executions
    );
    assert_eq!(
        after.scheduled, before.scheduled,
        "the refused deliveries must leave nothing scheduled, got {}",
        after.scheduled
    );

    // healed: the same two-step run completes exactly once
    wait_waiting_task(&engine, &pid, &step1).await;
    drive_to_settled(&engine, &pid, &step1).await;
    assert_settled(&engine, &pid).await;
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "the head act ran once"
    );
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0],
        "the forward act ran once"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × 重复消息 — the duplicate delivery is refused without effect.
// ---------------------------------------------------------------------------

/// The legacy cell delivered `Remove` twice and pinned that the second delivery
/// decided nothing further (one act removed, no duplicate task, the sibling
/// untouched). The kernel does not serve `Remove`, but the delivery is still a
/// decision the kernel refuses: both deliveries get the same verdict, and each
/// leaves the aggregate exactly as it was — the version, the executions and the
/// scheduled set after the second delivery are the ones after the first — so the
/// duplicate is a no-op for a structural reason (the refusal precedes the
/// aggregate) rather than a guard that happens to hold for the state.
///
/// The run is then driven through both steps: a refused duplicate may not have
/// propagated or scheduled anything, so the normal path still completes once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_remove_delivery_is_refused_without_effect() {
    bounded(
        "a_duplicate_remove_delivery_is_refused_without_effect",
        a_duplicate_remove_delivery_is_refused_without_effect_inner(),
    )
    .await;
}

async fn a_duplicate_remove_delivery_is_refused_without_effect_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, step1) = waiting_head(&engine).await;
    let before = settled_shape(&engine, &pid).await;

    let first = refused_remove(&engine, &pid, &step1).await;
    let after_first = settled_shape(&engine, &pid).await;
    assert_eq!(
        after_first.version, before.version,
        "the first refusal must consume nothing"
    );
    assert_eq!(
        after_first.executions, before.executions,
        "the first refusal must not decide anything, got {}",
        after_first.executions
    );
    assert_eq!(
        after_first.scheduled, before.scheduled,
        "the first refusal must not schedule anything, got {}",
        after_first.scheduled
    );

    // the duplicate: the same delivery, on the same waiting act
    let second = refused_remove(&engine, &pid, &step1).await;
    println!("remove refused twice: {first} / {second}");
    assert_eq!(
        second, first,
        "the duplicate must be refused with the same verdict"
    );
    let after_second = settled_shape(&engine, &pid).await;
    println!(
        "the decision point both refusals left: {} | {}",
        after_second.version, after_second.executions
    );
    assert_eq!(
        after_second.version, after_first.version,
        "the duplicate must not move the version the first refusal left"
    );
    assert_eq!(
        after_second.executions, after_first.executions,
        "the duplicate must not move the executions, got {}",
        after_second.executions
    );
    assert_eq!(
        after_second.scheduled, after_first.scheduled,
        "the duplicate must not schedule anything, got {}",
        after_second.scheduled
    );

    // nothing moved in the read model either: the head act still waits, and the
    // forward act was never dispatched by either refusal
    let proc = read_proc(&engine, &pid).await;
    assert!(
        waiting_task(&proc, &step1).is_some(),
        "the refusals must leave the head act waiting, got {:?}",
        task_snapshot(&proc)
    );
    assert!(
        waiting_task(&proc, "step2").is_none(),
        "a refused `Remove` must not propagate to the sibling, got {:?}",
        task_snapshot(&proc)
    );

    // and the run still finishes normally through both steps
    drive_to_settled(&engine, &pid, &step1).await;
    assert_settled(&engine, &pid).await;
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "the head act ran once"
    );
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0],
        "the forward act ran once"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × 完成清理 — a finished run is swept with its message and delivery
// rows.
// ---------------------------------------------------------------------------

/// The legacy cell pinned that a finished process is cleaned up together with
/// the message and delivery rows its ack channel stored, and that no in-flight
/// emission re-creates them afterwards. The kernel keeps the same two facts,
/// reached through the run's own act rather than the action: an ack channel
/// handed the act's `Created` stores a canonical message row plus that
/// channel's delivery row, the row stays open while the act runs, and the act's
/// terminal state *closes* it (`close_deliveries`) — which is what lets the
/// finished run be marked removable and swept, message and delivery rows
/// included.
///
/// So the cell proves the row set exists first (message and delivery both
/// present, the delivery still open), then that the sweep leaves neither behind
/// and that a late emission of the finished run cannot resurrect either. The
/// legacy cell's terminal-event drain is what this closing rule replaces: a
/// delivery that reached the store after its task closed is born `Completed`
/// there, so it can never hold the run out of the sweeper.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_run_is_swept_with_its_message_and_delivery_rows() {
    bounded(
        "a_completed_run_is_swept_with_its_message_and_delivery_rows",
        a_completed_run_is_swept_with_its_message_and_delivery_rows_inner(),
    )
    .await;
}

async fn a_completed_run_is_swept_with_its_message_and_delivery_rows_inner() {
    let engine = engine_on(FaultKv::new().store()).await;

    // an ack channel: every message it is handed is stored as the canonical
    // message row plus this channel's own delivery row, and that row stays open
    // until the act it belongs to closes (there is no client here to ack it)
    let ack = engine.channel_with_options(&ChannelOptions {
        id: "chan-clean".to_string(),
        ack: true,
        ..Default::default()
    });
    ack.on_message(|_| async {});

    let created = capture_created(&engine, "act1");
    let pid = start_run(&engine, &one_step_irq("act1")).await;
    let (announced, tid) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(tid, "s1", "the act of a `uses` step is the step's own node");
    wait_waiting_task(&engine, &pid, &tid).await;

    // the running act's message is stored, and its delivery is still open
    flush(&engine).await;
    let store = engine.runtime().store();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    // references, so the poll closures below capture a copy of the borrow
    // instead of moving the query and the store handle out of this scope
    let (store, q) = (&store, &q);
    let what = format!("the ack channel's message row for {pid} to be stored");
    poll_until(&what, || async {
        !store.messages().query(q).await.unwrap().rows.is_empty()
    })
    .await;
    let deliveries = store.deliveries().query(q).await.unwrap().rows;
    assert!(
        !deliveries.is_empty(),
        "the ack channel's delivery of the act's message must be stored while the act runs"
    );
    assert!(
        deliveries
            .iter()
            .all(|d| d.status != DeliveryStatus::Completed),
        "the delivery must still be open while the act it carries runs: {deliveries:?}"
    );

    // the run finishes through its act: the terminal task closes that delivery,
    // and the finished run hands every row to the sweeper
    complete_act(&engine, &pid, &tid).await.unwrap();
    wait_completed(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;

    // and the removed rows stay removed: a late emission of the finished run
    // must not re-create them behind the sweep
    assert_stays_empty("the swept run's message and delivery rows", 10, || async {
        (
            store.messages().query(q).await.unwrap().rows.len(),
            store.deliveries().query(q).await.unwrap().rows.len(),
        )
    })
    .await;
    engine.close().await;
}
