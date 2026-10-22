//! REMOVE reliability cells: the `Remove` client action under a crash
//! and reload, a store outage, a duplicate delivery, and the sweep of a
//! finished run's own message and delivery rows.
//!
//! The kernel serves `Remove` as the legacy engine did: the wire decision is
//! durable (the `__action` fact and its `ApplyAction` operation commit before
//! the act is decided), the act is decided `Removed`, its flow continues —
//! `Removed` schedules the node's successor exactly like the legacy
//! `push_next` — and the run's rows and aggregate carry the one decision. The
//! cells pin what a crash, an outage and a redelivery do to that decision:
//!
//! - a removal survives a crash and reload: the reloaded run still holds the
//!   removed act, one generation, and the continued flow is drivable once;
//! - an outage surfaces the store's own failure to the client and decides
//!   nothing half-way: the healed run is exactly where the wait left it, and
//!   the retried removal then lands once;
//! - a duplicate delivery of a decided removal is refused (`ActionNotAllowed`
//!   on the settled act) and moves nothing;
//! - a run finished by a removal is swept with its message and delivery rows,
//!   and no late emission re-creates them.

use std::future::Future;
use std::time::Instant;

use serial_test::serial;

use crate::ChannelOptions;
use crate::scheduler::core::model::task_graph::TaskState;
use crate::store::data::DeliveryStatus;

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
        step1,
        tid_of(&pid, "step1", 0),
        "the act of a `uses` step is the step's own instance"
    );
    wait_waiting_task(engine, &pid, "step1").await;
    flush(engine).await;
    (pid, step1)
}

/// Deliver one `Remove` and wait until it is decided: the act the delivery
/// names ends `Removed`, exactly once.
async fn removed(engine: &Engine, pid: &str, tid: &str) {
    act(engine, pid, tid, EventAction::Remove, Vars::new())
        .await
        .unwrap();
    let what = format!("{tid} of {pid} to be removed");
    poll_until(&what, || async {
        let snapshot = engine
            .runtime()
            .kernel()
            .snapshot(pid)
            .await
            .unwrap()
            .expect("the aggregate of a live run exists");
        snapshot
            .tasks
            .iter()
            .any(|(id, task)| id.wire(pid) == tid && task.state == TaskState::Removed)
    })
    .await;
}

/// One run's durable decision point, as comparable pieces. A client delivery
/// the kernel refuses must leave every piece identical, so each piece names
/// exactly what would have moved had the delivery been served.
struct Durable {
    /// The aggregate version: a refusal that landed a commit would move it.
    version: String,
    /// The sorted `(node, generation, state)` set: what the run has decided.
    tasks: String,
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
    let mut tasks: Vec<String> = snapshot
        .tasks
        .iter()
        .map(|(id, task)| format!("{}#{}={:?}", id.node.as_str(), id.generation, task.state))
        .collect();
    tasks.sort();
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
        tasks: tasks.join(","),
        scheduled: scheduled.join(","),
    }
}

/// [`durable_shape`] once the run stops moving: the same shape on three
/// consecutive ticks. The driver's own trailing operation — the delivery
/// confirmation of the act's `Created` — lands just after the client has seen
/// that message, so a decision point has to be read from a settled aggregate;
/// a shape taken mid-operation would measure the driver's own writes instead of the
/// delivery under test.
async fn settled_shape(engine: &Engine, pid: &str) -> Durable {
    let deadline = Instant::now() + WAIT;
    let mut last = durable_shape(engine, pid).await;
    let mut stable = 0;
    loop {
        tokio::time::sleep(TICK).await;
        let now = durable_shape(engine, pid).await;
        if now.version == last.version && now.tasks == last.tasks && now.scheduled == last.scheduled
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
            last.tasks,
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
    let node = super::node_of(&snapshot, pid, tid);
    let mut generations: Vec<u64> = snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == node)
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
    assert_eq!(
        step2,
        tid_of(pid, "step2", 0),
        "the completion propagates the next node"
    );
    wait_waiting_task(engine, pid, "step2").await;
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
// REMOVE × reload — a decided removal is durable: the reload finds it, once.
// ---------------------------------------------------------------------------

/// The legacy cell pinned a `Remove` replayed exactly once by recovery on
/// reload, and the removal's own durable act row (`Removed`). The kernel's
/// removal is the same durable decision: the wire fact and its operation
/// commit before the act is decided, so the reloaded run still holds the
/// removed act — one generation, never re-decided — and the flow the removal
/// pushed forward (`Removed` schedules the successor, the legacy `push_next`)
/// is drivable exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_removed_act_survives_a_reload_and_runs_forward_once() {
    bounded(
        "sch_remove_removed_act_survives_a_reload_and_runs_forward_once",
        a_removed_act_survives_a_reload_and_runs_forward_once_inner(),
    )
    .await;
}

async fn a_removed_act_survives_a_reload_and_runs_forward_once_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;

    // the propagated act is registered before the removal, so the flow the
    // removal pushes forward cannot be missed
    let next = capture_created(&engine, "act2");
    let (pid, step1) = waiting_head(&engine).await;

    // the removal: the act is decided `Removed`, the flow continues to step2
    removed(&engine, &pid, &step1).await;
    let (_, step2) = expect_created(&next).await;
    assert_eq!(
        step2,
        tid_of(&pid, "step2", 0),
        "the removed act's flow continues to the next node"
    );
    wait_waiting_task(&engine, &pid, "step2").await;
    flush(&engine).await;

    // the crash and the reload: the removal is durable — the reloaded run
    // holds the removed act (one generation) and the waiting successor
    let reloaded = restart(&engine, store.clone()).await;
    wait_waiting_task(&reloaded, &pid, "step2").await;
    let snapshot = reloaded
        .runtime()
        .kernel()
        .snapshot(&pid)
        .await
        .unwrap()
        .expect("the reloaded aggregate exists");
    assert!(
        snapshot
            .tasks
            .iter()
            .any(|(id, task)| id.wire(&pid) == step1 && task.state == TaskState::Removed),
        "the reload must hold the removal it committed: {:?}",
        snapshot.tasks
    );
    assert_eq!(
        generations(&reloaded, &pid, &step1).await,
        vec![0],
        "the reload must not re-decide the removed act (no rewind, no second generation)"
    );

    // the continued flow finishes once
    complete_act(&reloaded, &pid, &step2).await.unwrap();
    wait_completed(&reloaded, &pid).await;
    assert_settled(&reloaded, &pid).await;
    assert_eq!(
        generations(&reloaded, &pid, "step2").await,
        vec![0],
        "the forward act ran once"
    );
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × store fault — an outage decides nothing half-way; the retry lands once.
// ---------------------------------------------------------------------------

/// The legacy cell failed the durable write of the removal's state once and
/// pinned the graceful degradation: the fault surfaced at the durability
/// barrier and the act was not left half-decided. The kernel's removal is a
/// committed decision, so an outage surfaces the store's own failure to the
/// client and commits nothing — the healed run is exactly where the wait left
/// it (the act still waits, no parked intent surfaces), and the retried
/// removal then lands exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_outage_decides_nothing_half_way_and_the_retry_lands_once() {
    bounded(
        "sch_remove_outage_decides_nothing_half_way_and_the_retry_lands_once",
        an_outage_decides_nothing_half_way_and_the_retry_lands_once_inner(),
    )
    .await;
}

async fn an_outage_decides_nothing_half_way_and_the_retry_lands_once_inner() {
    let kv = OutageKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, step1) = waiting_head(&engine).await;
    let before = settled_shape(&engine, &pid).await;

    // the store goes away
    kv.down();

    // the removal's commit is the store's to refuse, and the client is told
    // *that* — the store's own failure, wrapped by the kernel in its runtime
    // error — rather than a silent success
    let attempts_before = kv.reads() + kv.writes();
    let failure = match act(&engine, &pid, &step1, EventAction::Remove, Vars::new()).await {
        Err(err) => err.to_string(),
        Ok(()) => panic!("a store that refuses a removal's commit must not report success"),
    };
    assert!(
        failure.contains("store is down"),
        "the failed removal must carry the store's own failure to the client, got {failure:?}"
    );
    assert!(
        kv.reads() + kv.writes() > attempts_before,
        "the removal must have attempted the store"
    );

    // the outage committed nothing: the healed run is exactly where the wait
    // left it, and no parked decision surfaces once the store is back
    kv.heal();
    let after = settled_shape(&engine, &pid).await;
    assert_eq!(
        after.version, before.version,
        "an outage must not move the run's version"
    );
    assert_eq!(
        after.tasks, before.tasks,
        "an outage must not decide anything, got {}",
        after.tasks
    );
    assert_eq!(
        after.scheduled, before.scheduled,
        "the failed removal must leave nothing scheduled, got {}",
        after.scheduled
    );

    // healed: the retried removal lands once, the flow continues, and the run
    // completes exactly once
    removed(&engine, &pid, &step1).await;
    let created = capture_created(&engine, "act2");
    let (_, step2) = expect_created(&created).await;
    assert_eq!(
        step2,
        tid_of(&pid, "step2", 0),
        "the removed act's flow continues to the next node"
    );
    wait_waiting_task(&engine, &pid, "step2").await;
    complete_act(&engine, &pid, &step2).await.unwrap();
    wait_completed(&engine, &pid).await;
    assert_settled(&engine, &pid).await;
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "the removed act kept its one generation"
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
// REMOVE × duplicate delivery — the duplicate delivery of a decided removal is refused.
// ---------------------------------------------------------------------------

/// The legacy cell delivered `Remove` twice and pinned that the second delivery
/// decided nothing further: one act removed, no duplicate task, the sibling
/// untouched. The kernel refuses the duplicate by the state table itself — a
/// `Removed` execution takes no further decision (`ActionNotAllowed`) — so the
/// duplicate moves neither the version nor the executions, and the run the
/// first removal pushed forward still finishes once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_duplicate_delivery_is_refused_without_effect() {
    bounded(
        "sch_remove_duplicate_delivery_is_refused_without_effect",
        a_duplicate_delivery_is_refused_without_effect_inner(),
    )
    .await;
}

async fn a_duplicate_delivery_is_refused_without_effect_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, step1) = waiting_head(&engine).await;

    // the first removal decides the act and pushes the flow forward
    let next = capture_created(&engine, "act2");
    removed(&engine, &pid, &step1).await;
    let (_, step2) = expect_created(&next).await;
    wait_waiting_task(&engine, &pid, "step2").await;
    flush(&engine).await;
    let after_first = settled_shape(&engine, &pid).await;

    // the duplicate: the same delivery, on the act the first one removed
    let error = match act(&engine, &pid, &step1, EventAction::Remove, Vars::new()).await {
        Err(err) => err.to_string(),
        Ok(()) => panic!("a decided act takes no further removal"),
    };
    assert!(
        error.contains("ActionNotAllowed"),
        "a decided act takes no further removal, got {error:?}"
    );
    println!("duplicate remove refused: {error}");
    let after_second = settled_shape(&engine, &pid).await;
    assert_eq!(
        after_second.version, after_first.version,
        "the duplicate must not move the version the first removal left"
    );
    assert_eq!(
        after_second.tasks, after_first.tasks,
        "the duplicate must not decide anything further, got {}",
        after_second.tasks
    );
    assert_eq!(
        after_second.scheduled, after_first.scheduled,
        "the duplicate must not schedule anything, got {}",
        after_second.scheduled
    );

    // the read model: the removed act kept its row, the sibling was never
    // duplicated by the redelivery
    let proc = read_proc(&engine, &pid).await;
    let states: Vec<String> = task_snapshot(&proc)
        .iter()
        .map(|(tid, state)| format!("{tid}={state}"))
        .collect();
    assert!(
        states
            .iter()
            .any(|s| s.contains(&format!("{step1}=removed"))),
        "the head act must read removed, got {states:?}"
    );
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "the duplicate must not grow a second generation"
    );

    // and the continued flow still finishes once
    complete_act(&engine, &pid, &step2).await.unwrap();
    wait_completed(&engine, &pid).await;
    assert_settled(&engine, &pid).await;
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0],
        "the forward act ran once"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × sweep — a finished run is swept with its message and delivery
// rows.
// ---------------------------------------------------------------------------

/// The legacy cell pinned that a finished process is cleaned up together with
/// the message and delivery rows its ack channel stored, and that no in-flight
/// emission re-creates them afterwards. The finish it waits for now goes
/// through the restored action itself: a `Remove` decides the act, the run
/// completes, and the same two facts hold — an ack channel handed the act's
/// `Created` stores a canonical message row plus that channel's delivery row,
/// the row stays open while the act runs, and the act's terminal state
/// (`Removed` included) *closes* it — which is what lets the finished run be
/// marked removable and swept, message and delivery rows included.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_completed_run_is_swept_with_its_message_and_delivery_rows() {
    bounded(
        "sch_remove_completed_run_is_swept_with_its_message_and_delivery_rows",
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
    assert_eq!(
        tid,
        tid_of(&pid, "s1", 0),
        "the act is addressed by its instance's wire"
    );
    wait_waiting_task(&engine, &pid, "s1").await;

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

    // the run finishes by removing its act: the terminal task closes that
    // delivery, and the finished run hands every row to the sweeper
    removed(&engine, &pid, &tid).await;
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
