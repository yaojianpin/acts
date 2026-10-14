//! ERROR reliability cells: the `Fail` client action (the wire `Error`) under a
//! store fault, a duplicate delivery of it, and `Fail` racing `Complete` on one
//! act.
//!
//! The kernel decides a client action by committing against the aggregate and
//! projects the rows from the committed snapshot afterwards. So the cells here
//! pin the two halves of the legacy writer contract on the surfaces the kernel
//! has:
//!
//! - a store fault the action's commit runs into is *surfaced to the client* —
//!   the action consumes nothing, so its retry decides exactly once;
//! - a store fault under the *projection* of a decided act is absorbed — the
//!   decision is the aggregate's and can never be undone (a reload can never
//!   resurrect the errored act as a waiting one) — and the read model converges
//!   or is swept with the run;
//! - a duplicate delivery is refused by the decision guard instead of being
//!   applied again, and a race converges on exactly one terminal decision.

use std::time::Duration;

use serial_test::serial;

use crate::TaskState;
use crate::runtime::core::model::execution_graph::ExecutionState;

use super::*;

/// Start one one-step irq run on `engine`, wait for its act to wait, and
/// answer `(pid, tid)` with every write of the wait settled — the state a
/// client action is aimed at.
async fn waiting_run(engine: &Engine) -> (String, String) {
    let created = capture_created(engine, "act1");
    let pid = start_run(engine, &one_step_irq("act1")).await;
    let (announced, tid) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(tid, "s1", "the act of a `uses` step is the step's own node");
    wait_waiting_task(engine, &pid, "s1").await;
    flush(engine).await;
    (pid, tid)
}

// ---------------------------------------------------------------------------
// ERROR × DB失败 — a fault the action's own commit runs into
// ---------------------------------------------------------------------------

/// A store refused at the moment of the `Fail` action must reach the client
/// instead of being swallowed: the commit is the durable truth, so a client
/// told "ok" would have lost its decision. The refusal consumes nothing (the
/// act is still waiting), and the retry decides it exactly once — the run
/// settles in the client's error and sweeps cleanly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_store_that_refuses_the_commit_surfaces_and_the_retry_decides_once() {
    bounded(
        "a_store_that_refuses_the_commit_surfaces_and_the_retry_decides_once",
        a_store_that_refuses_the_commit_surfaces_and_the_retry_decides_once_inner(),
    )
    .await;
}

async fn a_store_that_refuses_the_commit_surfaces_and_the_retry_decides_once_inner() {
    let kv = OutageKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, tid) = waiting_run(&engine).await;

    // the store goes away: every read and write the action needs fails
    kv.down();
    let before = kv.reads() + kv.writes();
    let refused = fail_act(&engine, &pid, &tid, "err1", "the act failed").await;
    assert!(
        refused.is_err(),
        "a store that refuses the action's commit must reach the client, got {refused:?}"
    );
    assert!(
        kv.reads() + kv.writes() > before,
        "the refused action must have attempted the store"
    );

    // the store heals; the retry decides the act exactly once
    kv.heal();
    fail_act(&engine, &pid, &tid, "err1", "the act failed")
        .await
        .unwrap();
    wait_completed(&engine, &pid).await;
    let proc = read_proc(&engine, &pid).await;
    assert!(
        proc.state().is_error(),
        "the run must settle in the client's error, got {}",
        proc.state()
    );
    assert_eq!(
        proc.task(&tid).map(|t| t.state()),
        Some(TaskState::Error),
        "the act must hold the decision"
    );
    assert_eq!(
        proc.task(&tid).and_then(|t| t.err()).map(|e| e.ecode),
        Some("err1".to_string()),
        "the decision must carry the client's code"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ERROR × DB失败 — a fault under the decided act's own row
// ---------------------------------------------------------------------------

/// A store fault under the projection of the errored act's row must not undo
/// or re-decide the action: the row is derived, so the decision stays the
/// aggregate's and a reload can never restore the errored act as a waiting
/// one.
///
/// Nothing repairs a row of a *settled* run (the projection is re-derived by a
/// later commit of the run, and recovery scans operations, never rows), so the
/// stale row is what the sweep reclaims a tick later: the read model either
/// converges or disappears with the run — it never keeps a settled run's act
/// reading as waiting. The run sweeps cleanly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_projection_keeps_the_decision_and_never_resurrects_the_act() {
    bounded(
        "a_failed_projection_keeps_the_decision_and_never_resurrects_the_act",
        a_failed_projection_keeps_the_decision_and_never_resurrects_the_act_inner(),
    )
    .await;
}

async fn a_failed_projection_keeps_the_decision_and_never_resurrects_the_act_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, tid) = waiting_run(&engine).await;

    // The fault lands on the act's own projected row: the action is accepted
    // (the row is derived, never the decision), and the failed write is the one
    // that would have carried the decision into the read model.
    let row_key = task_data_key("tasks", &pid, &tid);
    kv.arm_key(&row_key);
    fail_act(&engine, &pid, &tid, "err1", "the act failed")
        .await
        .unwrap();
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(row_key),
        "the fault must land on the errored act's own row"
    );

    // the decision is durable: the aggregate holds a terminal act
    wait_completed(&engine, &pid).await;
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(&pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    let act = snapshot
        .executions
        .iter()
        .find(|(id, _)| id.task_id == tid)
        .map(|(_, execution)| execution)
        .expect("the act's execution");
    assert!(
        act.state.is_terminal(),
        "the decided act must be terminal in the aggregate, got {:?}",
        act.state
    );

    // the read model converges to the decision (or the run is swept with the
    // stale row; either way the act never keeps reading as waiting)
    let what = format!("the decided act of {pid} to stop reading as waiting");
    poll_until_dumped(
        &what,
        || async {
            match proc_row(&engine, &pid).await {
                Some(proc) => proc
                    .task(&tid)
                    .is_none_or(|t| t.state() != TaskState::Interrupt),
                None => true,
            }
        },
        || dump_state(&engine, &pid),
    )
    .await;

    sweep_until_gone(&engine, &pid).await;

    // a reload after the fault observes no waiting act at all
    let reloaded = restart(&engine, store.clone()).await;
    if let Some(proc) = proc_row(&reloaded, &pid).await {
        let waiting: Vec<String> = proc
            .tasks()
            .iter()
            .filter(|t| t.state().is_interrupted())
            .map(|t| t.id.clone())
            .collect();
        assert!(
            waiting.is_empty(),
            "a reload must not resurrect the decided act: {waiting:?}"
        );
    }
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// ERROR × 重复投递 — the duplicate is refused, not re-applied
// ---------------------------------------------------------------------------

/// Delivering the same `Fail` twice must decide the act exactly once: the
/// second delivery is refused by the decision guard (the act is already
/// `Error`) and changes nothing — one task for the act, one error, no further
/// propagation — and the run settles in the very error the client was told
/// about instead of being laundered into a completion.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_fail_delivery_is_refused_without_effect() {
    bounded(
        "a_duplicate_fail_delivery_is_refused_without_effect",
        a_duplicate_fail_delivery_is_refused_without_effect_inner(),
    )
    .await;
}

async fn a_duplicate_fail_delivery_is_refused_without_effect_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, tid) = waiting_run(&engine).await;

    fail_act(&engine, &pid, &tid, "err1", "the act failed")
        .await
        .unwrap();
    // let the first decision settle durably before the duplicate arrives, so
    // the duplicate is a retry and not a race with the first delivery
    wait_completed(&engine, &pid).await;
    flush(&engine).await;

    let before = read_proc(&engine, &pid).await;
    assert_eq!(tasks_of(&before, &tid).len(), 1, "one task for the act");
    assert_eq!(
        before.task(&tid).map(|t| t.state()),
        Some(TaskState::Error),
        "the first delivery leaves the act errored"
    );

    let duplicate = fail_act(&engine, &pid, &tid, "err1", "the act failed").await;
    assert!(
        duplicate.is_err(),
        "a duplicate fail must never be applied again, got {duplicate:?}"
    );

    // the verdict is the only thing the duplicate produced: the tree and the
    // run's outcome are exactly what the first delivery left
    flush(&engine).await;
    let after = read_proc(&engine, &pid).await;
    assert_eq!(
        tasks_of(&after, &tid).len(),
        1,
        "no second task for the act"
    );
    assert_eq!(
        after.task(&tid).map(|t| t.state()),
        Some(TaskState::Error),
        "the duplicate may not move the decided act"
    );
    assert_eq!(
        task_snapshot(&after),
        task_snapshot(&before),
        "a refused duplicate must leave the tree untouched"
    );
    assert!(after.state().is_error(), "the run settles in its error");
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ERROR × 并发 — Fail racing Complete on one act
// ---------------------------------------------------------------------------

/// `Fail` and `Complete` fired at the same waiting act must converge on one
/// decision: every racer gets a verdict, exactly one applies (the loser is
/// refused by the decision guard), the act holds exactly one terminal state,
/// exactly one task exists for it, no task of the run is left non-terminal,
/// and the run settles once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn fail_racing_complete_converges_on_one_terminal_decision() {
    bounded(
        "fail_racing_complete_converges_on_one_terminal_decision",
        fail_racing_complete_converges_on_one_terminal_decision_inner(),
    )
    .await;
}

async fn fail_racing_complete_converges_on_one_terminal_decision_inner() {
    for round in 0..3 {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, tid) = waiting_run(&engine).await;

        // the race: both deliveries target the same waiting act at once
        let (fail, complete) = tokio::join!(
            fail_act(&engine, &pid, &tid, "err1", "the act failed"),
            complete_act(&engine, &pid, &tid),
        );
        assert!(
            fail.is_ok() ^ complete.is_ok(),
            "round {round}: exactly one racer may decide the act, got fail={fail:?} complete={complete:?}"
        );

        wait_completed(&engine, &pid).await;
        let proc = read_proc(&engine, &pid).await;
        let state = proc.task(&tid).map(|t| t.state()).expect("the act's task");
        assert!(
            state.is_completed(),
            "round {round}: the act must hold one terminal state, got {state}"
        );
        assert_eq!(
            tasks_of(&proc, &tid).len(),
            1,
            "round {round}: one task for the act"
        );
        let unfinished: Vec<String> = proc
            .tasks()
            .iter()
            .filter(|t| !t.state().is_completed())
            .map(|t| format!("{}:{}", t.id, t.state()))
            .collect();
        assert!(
            unfinished.is_empty(),
            "round {round}: no task may be left non-terminal: {unfinished:?}"
        );
        flush(&engine).await;
        let rows = engine
            .runtime()
            .store()
            .tasks()
            .query(&Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone()))))
            .await
            .unwrap()
            .rows;
        assert_eq!(
            rows.iter().filter(|row| row.tid == tid).count(),
            1,
            "round {round}: exactly one durable row for the act"
        );
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}

// ---------------------------------------------------------------------------
// ERROR × restart — a decided fail is never re-applied
// ---------------------------------------------------------------------------

/// A restart over the same store must not re-apply a fail the client was
/// already told about: the recovered run is terminal in its error, no fresh
/// `Created` is announced for the decided act, and the recovered rows sweep
/// cleanly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_does_not_reapply_a_decided_fail() {
    bounded(
        "a_restart_does_not_reapply_a_decided_fail",
        a_restart_does_not_reapply_a_decided_fail_inner(),
    )
    .await;
}

async fn a_restart_does_not_reapply_a_decided_fail_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, tid) = waiting_run(&engine).await;

    fail_act(&engine, &pid, &tid, "err1", "the act failed")
        .await
        .unwrap();
    wait_completed(&engine, &pid).await;
    flush(&engine).await;

    let reloaded = restart(&engine, store.clone()).await;
    // the recovered engine announces nothing for the decided act: a fresh
    // `Created` would invite the client to decide an act it was already told
    // the verdict of
    let resurrected = capture_created(&reloaded, "act1");
    let announced = tokio::time::timeout(Duration::from_millis(2000), resurrected.recv()).await;
    assert!(
        announced.is_err(),
        "recovery must never announce the decided act again: {:?}",
        announced.ok()
    );

    let proc = proc_row(&reloaded, &pid).await;
    if let Some(proc) = &proc {
        assert!(
            proc.state().is_error(),
            "the recovered run must hold its error, got {}",
            proc.state()
        );
        assert_eq!(
            proc.task(&tid).map(|t| t.state()),
            Some(TaskState::Error),
            "the recovered act must hold its decision"
        );
    } else {
        // a settled and fully delivered run is swept; its aggregate stays, and
        // that is what the decision lives in
        assert!(
            engine_state(&reloaded, &pid, &tid).await.is_some(),
            "the aggregate must outlive the swept rows"
        );
    }
    let decided = engine_state(&reloaded, &pid, &tid).await;
    assert_eq!(
        decided,
        Some(ExecutionState::Failed),
        "the recovered aggregate must keep the act failed"
    );
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

/// The aggregate's state for the node `tid` of one run, if the aggregate is
/// still there.
async fn engine_state(engine: &Engine, pid: &str, tid: &str) -> Option<ExecutionState> {
    engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .and_then(|snapshot| {
            snapshot
                .executions
                .iter()
                .find(|(id, _)| id.task_id == tid)
                .map(|(_, execution)| execution.state)
        })
}
