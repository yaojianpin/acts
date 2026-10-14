//! CANCEL reliability cells: the `Cancel` client action — the decision that
//! closes a finished step `Backed` and re-enters it with a fresh generation —
//! under a duplicate delivery, a store fault, a race with `Next`, and a crash
//! between its intent and its effect.
//!
//! The kernel decides a client action by committing against the aggregate and
//! projects the rows from the committed snapshot afterwards. Its `Cancel` is
//! strict: it needs a *finished* step with live forward work
//! (`validate_cancel_forward_work`), its own commit closes the source
//! generation `Backed` and creates the durable `ReenterTask` operation that
//! plans the redo. So the cells here pin, on the surfaces the kernel has:
//!
//! - "exactly one redo" is a *generation* fact — the aggregate's generations
//!   for the model node, plus the node's own `Created` message — never a task
//!   count (the read model keeps exactly one row per model node, so a count
//!   cannot see a rewind at all);
//! - a store fault the cancel's own durable commit runs into is surfaced to
//!   the client and consumes nothing, so the retry rewinds exactly once;
//! - a duplicate delivery is refused by the guard — the newest generation of
//!   the node is the redo, still waiting, and only a *finished* step is
//!   cancelable — so a second delivery can never add a second rewind;
//! - a `Cancel` racing the `Next` that finishes the same node converges on one
//!   decision, with a redo iff the cancel won;
//! - a cancel whose durable intent landed but whose effect never committed is
//!   replayed by recovery exactly once.

use std::time::{Duration, Instant};

use serial_test::serial;

use crate::ActError;
use crate::scheduler::core::model::execution_graph::ExecutionState;

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Start a two-step irq run, finish `step1`'s act, and wait until `step2`'s
/// act waits: the state a `Cancel` of the head step is aimed at — that step's
/// own work is done and the chain forward of it is live.
///
/// The head act's `Created` is consumed here; a cell that wants the *rewind's*
/// own `Created` registers a fresh [`capture_created`] for the head act's key
/// afterwards.
async fn waiting_chain(engine: &Engine) -> (String, String) {
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

    let created = capture_created(engine, "act2");
    complete_act(engine, &pid, &step1).await.unwrap();
    let (announced, step2) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(step2, "step2");
    wait_waiting_task(engine, &pid, &step2).await;
    flush(engine).await;
    (pid, step1)
}

/// Complete whatever of the two-step run waits, by node, until the run is
/// done. A rewind leaves the *redo* acts waiting, so the cell drives those
/// too; the client names only nodes, and the newest generation is the one an
/// action targets.
async fn finish_by_hand(engine: &Engine, pid: &str) {
    // The drive-by-hand bound is deliberately longer than the harness `WAIT`:
    // the full suite runs this cell alongside ~850 others, so the driver's
    // ticks and the completion's hops can be starved well past the stand-alone
    // cadence. The assertion is the same either way (the run must settle); the
    // dump is what a genuine stall ships home.
    let deadline = Instant::now() + WAIT * 3;
    loop {
        if engine
            .runtime()
            .kernel()
            .process_completed(pid)
            .await
            .unwrap_or(false)
        {
            return;
        }
        let proc = read_proc(engine, pid).await;
        for node in ["step1", "step2"] {
            if waiting_task(&proc, node).is_some() {
                let _ = complete_act(engine, pid, node).await;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the run {pid} never settled: {}",
            dump_state(engine, pid).await
        );
        tokio::time::sleep(TICK).await;
    }
}

/// The ascending generations the aggregate holds for one model node: how many
/// times the node has been instantiated. A rewind adds exactly one.
async fn generations(engine: &Engine, pid: &str, tid: &str) -> Vec<u64> {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    let mut generations: Vec<u64> = snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == tid)
        .map(|id| id.generation)
        .collect();
    generations.sort_unstable();
    generations
}

/// The aggregate's state for one generation of one node.
async fn generation_state(
    engine: &Engine,
    pid: &str,
    tid: &str,
    generation: u64,
) -> Option<ExecutionState> {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    snapshot
        .executions
        .iter()
        .find(|(id, _)| id.task_id == tid && id.generation == generation)
        .map(|(_, execution)| execution.state)
}

/// Whether the aggregate still holds a non-terminal operation for `tid`: the
/// durable intent a crash left for recovery to drive.
async fn has_pending_operation(engine: &Engine, pid: &str, tid: &str) -> bool {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    snapshot
        .operations
        .values()
        .any(|operation| operation.id.task_id == tid && !operation.is_terminal())
}

// ---------------------------------------------------------------------------
// CANCEL × 重复消息 — a duplicate Cancel delivery is refused without a second
// rewind.
// ---------------------------------------------------------------------------

/// Delivering the same `Cancel` twice must rewind the head step exactly once:
/// the first delivery closes the finished step `Backed` and re-enters it — one
/// fresh generation, one fresh act `Created` — and the second delivery is
/// refused, because the newest generation of the node is now the redo itself,
/// still waiting on its own act, and only a *finished* step is cancelable.
///
/// The legacy duplicate slipped past a guard checked on the parent step (which
/// stays finished after the first cancel) and created a second redo path; the
/// kernel's guard reads the target generation, and the redo generation it
/// faces is live, so the refusal is structural rather than incidental. The
/// refused duplicate consumes nothing: no third generation, no second redo
/// act, and the run still finishes through the one redo the client drove.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_cancel_is_refused_and_the_rewind_runs_once() {
    bounded(
        "a_duplicate_cancel_is_refused_and_the_rewind_runs_once",
        a_duplicate_cancel_is_refused_and_the_rewind_runs_once_inner(),
    )
    .await;
}

async fn a_duplicate_cancel_is_refused_and_the_rewind_runs_once_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the rewind's own `Created`, registered before the cancel that asks for it
    let rewind = capture_created(&engine, "act1");
    cancel_act(&engine, &pid, &step1).await.unwrap();
    let (announced, redone) = expect_created(&rewind).await;
    assert_eq!(announced, pid, "the rewind names its own run");
    assert_eq!(
        redone, step1,
        "the rewind re-enters the node the client named"
    );
    wait_waiting_task(&engine, &pid, &step1).await;

    // every later `Created` of the node is observed: the duplicate must not
    // announce one
    let extra = capture_created(&engine, "act1");
    let duplicate = cancel_act(&engine, &pid, &step1).await;
    assert!(
        duplicate.is_err(),
        "a duplicate cancel must be refused, got {duplicate:?}"
    );
    let announced = tokio::time::timeout(Duration::from_millis(2000), extra.recv()).await;
    assert!(
        announced.is_err(),
        "the refused duplicate must not announce a second rewind: {:?}",
        announced.ok()
    );

    // exactly one rewind: one fresh generation for the head and one for the
    // forward chain the rewind closed over
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0, 1],
        "the head step must be redone exactly once"
    );
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0, 1],
        "the forward chain must be recreated exactly once"
    );

    finish_by_hand(&engine, &pid).await;
    assert_settled(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// CANCEL × DB失败 — a fault the cancel's own durable commit runs into
// ---------------------------------------------------------------------------

/// A store fault under the cancel's own durable commit must reach the client
/// instead of being swallowed: that commit *is* the durable decision, so a
/// client told "ok" would have lost a rewind it was promised. The refusal
/// consumes nothing — the aggregate still holds the finished head step and no
/// rewind generation — and the retry (the fault is one-shot) rewinds exactly
/// once, so the run settles and sweeps with one redo and no duplicated work.
///
/// The kernel has no per-act durable write: where the legacy cell broke the
/// cancelled act's own state write, the nearest durable unit is the run's
/// aggregate commit, which is what carries the cancelled act's state and the
/// rewind operation together. Breaking it is therefore breaking the cancel's
/// own durable step.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_store_that_refuses_the_cancel_commit_surfaces_and_the_rewind_lands_once() {
    bounded(
        "a_store_that_refuses_the_cancel_commit_surfaces_and_the_rewind_lands_once",
        a_store_that_refuses_the_cancel_commit_surfaces_and_the_rewind_lands_once_inner(),
    )
    .await;
}

async fn a_store_that_refuses_the_cancel_commit_surfaces_and_the_rewind_lands_once_inner() {
    let kv = FaultKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the run's durable truth — the aggregate the cancel decides in — breaks
    // exactly once, on the cancel's own commit
    let aggregate = aggregate_key(&pid);
    kv.arm_key(&aggregate);
    let rewind = capture_created(&engine, "act1");
    let refused = cancel_act(&engine, &pid, &step1).await;
    match &refused {
        Err(ActError::Store(_)) => {}
        other => panic!("the refused durable commit must reach the client, got {other:?}"),
    }
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(aggregate),
        "the fault must land on the run's aggregate commit"
    );

    // the refusal consumed nothing: no rewind, the head step still finished
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "a refused cancel must not rewind"
    );
    assert_eq!(
        generation_state(&engine, &pid, &step1, 0).await,
        Some(ExecutionState::Completed),
        "the head step is still the finished one the retry cancels"
    );

    // the healed retry lands one rewind
    cancel_act(&engine, &pid, &step1).await.unwrap();
    let (_, redone) = expect_created(&rewind).await;
    assert_eq!(
        redone, step1,
        "the retry re-enters the node the client named"
    );
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0, 1],
        "exactly one rewind after the healed fault"
    );
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0, 1],
        "the forward chain must be recreated exactly once"
    );

    finish_by_hand(&engine, &pid).await;
    assert_settled(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// CANCEL × 并发 — Cancel racing the Next that finishes the same node
// ---------------------------------------------------------------------------

/// `Cancel` and `Next` fired at the same node in the same instant must
/// converge on exactly one decision: `Next` always applies (it only needs the
/// waiting act), and the cancel applies *iff* it re-planned after the
/// completion had propagated the next act — a cancel needs the head step
/// finished *and* live forward work, and the completion is what finishes it,
/// while the forward step only exists once the completion's propagation
/// created it. So the head holds exactly one terminal decision (`Backed` with
/// a redo, or `Completed` alone), the rewind happened iff the cancel won, and
/// the run settles once either way. Several rounds cover the interleavings.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn cancel_racing_next_converges_on_one_terminal_decision() {
    bounded(
        "cancel_racing_next_converges_on_one_terminal_decision",
        cancel_racing_next_converges_on_one_terminal_decision_inner(),
    )
    .await;
}

async fn cancel_racing_next_converges_on_one_terminal_decision_inner() {
    for round in 0..3 {
        let engine = engine_on(FaultKv::new().store()).await;
        let created = capture_created(&engine, "act1");
        let pid = start_run(&engine, &two_step_irq_workflow()).await;
        let (announced, step1) = expect_created(&created).await;
        assert_eq!(
            announced, pid,
            "the message names the run it was created for"
        );
        assert_eq!(step1, "step1");
        wait_waiting_task(&engine, &pid, &step1).await;
        flush(&engine).await;

        // the race: both deliveries target the waiting head node at once
        let rewind = capture_created(&engine, "act1");
        let (next, cancel) = tokio::join!(
            complete_act(&engine, &pid, &step1),
            cancel_act(&engine, &pid, &step1),
        );
        assert!(
            next.is_ok(),
            "round {round}: completing the waiting act is unconditional, got {next:?}"
        );
        let redone = cancel.is_ok();
        println!(
            "round {round}: next={} cancel={}",
            outcome(&next),
            outcome(&cancel)
        );

        if redone {
            let (_, tid) = expect_created(&rewind).await;
            assert_eq!(
                tid, step1,
                "round {round}: a winning cancel re-enters the head node"
            );
        } else {
            let announced = tokio::time::timeout(Duration::from_millis(2000), rewind.recv()).await;
            assert!(
                announced.is_err(),
                "round {round}: a refused cancel must not rewind: {:?}",
                announced.ok()
            );
        }

        // the completion propagates the forward node; it exists in either
        // interleaving (a winning cancel closes it and recreates it)
        let what = format!("round {round}: the head step's forward node to exist");
        poll_until(&what, || async {
            !generations(&engine, &pid, "step2").await.is_empty()
        })
        .await;

        assert_eq!(
            generation_state(&engine, &pid, &step1, 0).await,
            Some(if redone {
                ExecutionState::Backed
            } else {
                ExecutionState::Completed
            }),
            "round {round}: the head must hold exactly one terminal decision"
        );
        let expected = if redone { vec![0, 1] } else { vec![0] };
        assert_eq!(
            generations(&engine, &pid, &step1).await,
            expected,
            "round {round}: a rewind iff the cancel won"
        );
        assert_eq!(
            generations(&engine, &pid, "step2").await,
            expected,
            "round {round}: the forward chain recreated iff the cancel won"
        );

        finish_by_hand(&engine, &pid).await;
        assert_settled(&engine, &pid).await;
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}

// ---------------------------------------------------------------------------
// CANCEL × restart — a cancel whose intent landed is re-applied exactly once
// ---------------------------------------------------------------------------

/// A `Cancel` whose durable intent landed but whose effect never committed (a
/// crash between the two, here a write cut) must be replayed by recovery
/// exactly once: the intent is the durable decision, and the hop that follows
/// it re-plans the action the client made — the head step `Backed` plus the
/// `ReenterTask` rewind — so the rewind survives the crash instead of being
/// silently dropped.
///
/// The cut is placed at the cancel's second durable write (the claim right
/// after the intent), so the intent is durable and `Pending` while the effect
/// is lost: exactly the state a crash between the two leaves. The engine is
/// closed before the store heals, so no live driver can apply the intent —
/// the boot of the fresh engine over the same store is what re-drives it.
/// Legacy's concern applies: the target act is already `Completed`, and a
/// terminal-state check that closes the record instead of replaying the
/// action would drop the cancel.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_whose_intent_landed_is_reapplied_exactly_once_on_recovery() {
    bounded(
        "a_cancel_whose_intent_landed_is_reapplied_exactly_once_on_recovery",
        a_cancel_whose_intent_landed_is_reapplied_exactly_once_on_recovery_inner(),
    )
    .await;
}

async fn a_cancel_whose_intent_landed_is_reapplied_exactly_once_on_recovery_inner() {
    let kv = CutKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the intent commit lands; the claim that follows it is cut, so the effect
    // never commits
    let before = kv.writes();
    kv.cut_at(before + 2);
    let lost = cancel_act(&engine, &pid, &step1).await;
    assert!(
        lost.is_err(),
        "a cut writer must reach the client, got {lost:?}"
    );
    assert_eq!(
        kv.refused(),
        1,
        "exactly the write after the intent must have been cut"
    );

    // the crash state: the intent is durable and undecided, its effect lost
    assert!(
        has_pending_operation(&engine, &pid, &step1).await,
        "the crash must leave the cancel's durable intent pending"
    );
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "the lost effect must not have rewound"
    );
    assert_eq!(
        generation_state(&engine, &pid, &step1, 0).await,
        Some(ExecutionState::Completed),
        "the head step is still finished, the state the replayed cancel reads"
    );

    // the crash and the replay: close before healing, then boot a fresh engine
    // over the durable state (its recovery pass is what re-drives the intent)
    engine.close().await;
    kv.heal();
    let reloaded = engine_on(store.clone()).await;

    let what = format!("the recovered cancel of {pid} to rewind the head step once");
    poll_until_dumped(
        &what,
        || async { generations(&reloaded, &pid, &step1).await == vec![0, 1] },
        || async {
            // the replayed intent's fate, not just the row: a cancel dropped by
            // the recovered hop shows up here as a pending intent that ran to
            // completion while the head step never moved
            format!(
                "{} | head gens={:?} state0={:?} intent-pending={}",
                dump_state(&reloaded, &pid).await,
                generations(&reloaded, &pid, &step1).await,
                generation_state(&reloaded, &pid, &step1, 0).await,
                has_pending_operation(&reloaded, &pid, &step1).await,
            )
        },
    )
    .await;
    assert_eq!(
        generations(&reloaded, &pid, &step1).await,
        vec![0, 1],
        "recovery must re-apply the cancel exactly once"
    );
    assert_eq!(
        generations(&reloaded, &pid, "step2").await,
        vec![0, 1],
        "the rewind's forward chain must be recreated exactly once"
    );
    assert_eq!(
        generation_state(&reloaded, &pid, &step1, 0).await,
        Some(ExecutionState::Backed),
        "the replayed cancel must close the finished head step Backed"
    );
    // the replayed rewind re-dispatches the head act, and the intent's
    // operation plus the `ReenterTask` it created are driven to completion —
    // not stranded pending
    wait_waiting_task(&reloaded, &pid, &step1).await;
    assert!(
        poll_settle(
            || async { !has_pending_operation(&reloaded, &pid, &step1).await },
            100,
        )
        .await,
        "the replayed intent must be settled, not left pending"
    );

    finish_by_hand(&reloaded, &pid).await;
    assert_settled(&reloaded, &pid).await;
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}
