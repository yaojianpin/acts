//! NEXT reliability cells: the `Next` client action's *propagation* — the hop
//! that decides a step `Completed` and fans the flow out into the step after it
//! — under a store fault on the step it schedules, a durable fault on the hop's
//! own commit, a crash that leaves the hop undecided, an already-applied hop a
//! recovery scan sees, a crash at each write boundary of the hop, and a
//! container that may not finish before its children.
//!
//! The kernel decides a client action in four writes against the aggregate —
//! the durable *intent* (the operation plus the private action fact its hop
//! replays), the claim that prepares it, the effect commit, and the close — and
//! projects the rows from those commits afterwards. The effect commit decides the
//! act and records the hop as its `action:` outbox record, and in the same batch
//! it seeds the `PropagateTask` operation that *is* the propagation: that hop,
//! with a claim, an effect commit and a close of its own, writes the `propagate:`
//! record and moves the step the flow schedules into `Ready` with an
//! `ExecuteTask` operation of its own. So a cell reads "exactly once" off
//! the surfaces that hold it: the aggregate's executions (one per model node and
//! generation) and its outbox records, whose keys are deterministic per
//! `(pid, tid, generation, sequence)` — a hop that applied twice shows up as a
//! second record or a second generation — and "the read model follows" off the
//! projected rows.
//!
//! Legacy's `next` bookkeeping is gone, and the cells that pinned it say so
//! where they stand: there is no writer `PropagationPhase` (the kernel's
//! equivalent is the operation's own state machine, and an applied hop leaves a
//! *closed* operation rather than an open record), and there is no `next` outbox
//! record the writer could lose and re-materialize (the kernel keeps the record
//! inside the aggregate, so one batch either carries the whole hop or none of
//! it).

use serial_test::serial;

use crate::ActError;
use crate::scheduler::core::model::clock::{Clock, SystemClock};
use crate::scheduler::core::model::execution_graph::ExecutionState;
use crate::scheduler::core::model::operation::OperationState;

use super::*;

/// The four writes of one client action's hop, in order: the durable intent,
/// the claim that prepares it, the effect commit (which decides the act and seeds
/// the propagation the flow runs next), and the close. They are the boundaries a
/// crash cell cuts the writer at.
const INTENT_WRITE: usize = 1;
const CLAIM_WRITE: usize = 2;
const EFFECT_WRITE: usize = 3;
const CLOSE_WRITE: usize = 4;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Start the two-step irq chain and answer `(pid, "step1")` once `step1`'s act
/// waits: the state a propagation is decided from, where the step it schedules is
/// still the `Created` execution the model import seeded.
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

/// Start the two-step irq chain, finish `step1`'s act, and answer
/// `(pid, "step1")` once `step2`'s act waits: the smallest run in which a
/// propagation is already *applied* — its child exists and its record is
/// written.
async fn waiting_chain(engine: &Engine) -> (String, String) {
    let created = capture_created(engine, "act1");
    let pid = start_run(engine, &two_step_irq_workflow()).await;
    let (announced, step1) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    wait_waiting_task(engine, &pid, &step1).await;

    let created = capture_created(engine, "act2");
    complete_act(engine, &pid, &step1).await.unwrap();
    let (_, step2) = expect_created(&created).await;
    assert_eq!(
        step2, "step2",
        "the propagation schedules the next model node"
    );
    wait_waiting_task(engine, &pid, &step2).await;
    flush(engine).await;
    (pid, step1)
}

/// Complete whatever of the chain waits, by node, until every execution of the
/// run is terminal. A recovery that re-drove the propagation leaves the child
/// act waiting, and the client — not the engine — is who completes it.
async fn finish_chain(engine: &Engine, pid: &str) {
    let what = format!("run {pid} to finish with the cell driving its acts");
    poll_until_dumped(
        &what,
        || async {
            if engine
                .runtime()
                .kernel()
                .process_completed(pid)
                .await
                .unwrap_or(false)
            {
                return true;
            }
            let Some(proc) = proc_row(engine, pid).await else {
                return false;
            };
            for node in ["step1", "step2"] {
                if waiting_task(&proc, node).is_some() {
                    let _ = complete_act(engine, pid, node).await;
                }
            }
            false
        },
        || dump_state(engine, pid),
    )
    .await;
}

/// The branched container every fan-out cell runs: one step with two branches,
/// each branch running its own irq act, joined when both branches settle.
///
/// The kernel's fan-out is read from the model (`model::step_branches`), so the
/// container is a step with explicit branches. The container packages' `in`/
/// `acts` expansion is *not* a model shape the kernel drives: `ctx.build_acts`
/// writes into the headless effect scope the package runner throws away, so a
/// `uses: acts.core.parallel` step with an `acts` array resolves the package and
/// auto-completes — no child execution, no act, no message.
fn branched_workflow() -> Workflow {
    Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|branch| {
                branch.with_id("b1").with_if("true").with_step(|inner| {
                    inner
                        .with_id("s1")
                        .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                })
            })
            .with_branch(|branch| {
                branch.with_id("b2").with_if("true").with_step(|inner| {
                    inner
                        .with_id("s2")
                        .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
                })
            })
    })
}

// ---------------------------------------------------------------------------
// The aggregate's own record — where "exactly once" is decided
// ---------------------------------------------------------------------------

/// The run's aggregate document. A settled run keeps it even after its rows are
/// swept, so a cell reads a finished run's decision here and never off a row.
async fn aggregate(
    engine: &Engine,
    pid: &str,
) -> crate::scheduler::core::commit::aggregate::AggregateSnapshot {
    engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the aggregate of {pid} must exist"))
}

/// The ascending generations the aggregate holds for one model node: how many
/// times the node was instantiated. A hop applied twice adds one.
async fn generations(engine: &Engine, pid: &str, tid: &str) -> Vec<u64> {
    let mut generations: Vec<u64> = aggregate(engine, pid)
        .await
        .executions
        .keys()
        .filter(|id| id.task_id == tid)
        .map(|id| id.generation)
        .collect();
    generations.sort_unstable();
    generations
}

/// The aggregate's state for the newest generation of one model node.
async fn decided_state(engine: &Engine, pid: &str, tid: &str) -> Option<ExecutionState> {
    aggregate(engine, pid)
        .await
        .executions
        .iter()
        .filter(|(id, _)| id.task_id == tid)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, execution)| execution.state)
}

/// The non-terminal operations the aggregate holds for one model node, in
/// sequence order: the durable remainder a crash left for recovery to drive.
async fn open_operations(engine: &Engine, pid: &str, tid: &str) -> Vec<OperationState> {
    let mut open: Vec<(u64, OperationState)> = aggregate(engine, pid)
        .await
        .operations
        .values()
        .filter(|operation| operation.id.task_id == tid && !operation.is_terminal())
        .map(|operation| (operation.id.sequence, operation.state.clone()))
        .collect();
    open.sort_by_key(|(sequence, _)| *sequence);
    open.into_iter().map(|(_, state)| state).collect()
}

/// [`open_operations`] as short, comparable names.
fn open_shapes(states: &[OperationState]) -> Vec<&'static str> {
    states
        .iter()
        .map(|state| match state {
            OperationState::Pending => "pending",
            OperationState::RetryScheduled { .. } => "retry-scheduled",
            OperationState::Claimed(_) => "claimed",
            OperationState::CommitPrepared(_) => "prepared",
            OperationState::Committed(_) => "committed",
            OperationState::Completed(_) => "completed",
            OperationState::EffectUnknown { .. } => "effect-unknown",
            OperationState::Quarantined { .. } => "quarantined",
            OperationState::Superseded { .. } => "superseded",
        })
        .collect()
}

/// How many outbox records of the run fall under `prefix`. A hop's key is
/// deterministic per `(pid, tid, generation, sequence)`, so a hop that applied
/// twice cannot hide behind the count.
async fn records_with_prefix(engine: &Engine, pid: &str, prefix: &str) -> usize {
    aggregate(engine, pid)
        .await
        .outbox
        .keys()
        .filter(|id| id.key.starts_with(prefix))
        .count()
}

/// The propagation hop's key family for one act: one `action` record for the
/// decision and one `propagate` record for the hop it set off.
fn next_records(pid: &str, tid: &str) -> (String, String) {
    (
        format!("action:{pid}:{tid}:"),
        format!("propagate:{pid}:{tid}:"),
    )
}

/// The propagation of `source` landed exactly once: one execution for the act,
/// the completion it holds, and exactly one record per hop of the decision.
async fn assert_propagation_once(engine: &Engine, pid: &str, source: &str) {
    let (action, propagate) = next_records(pid, source);
    assert_eq!(
        generations(engine, pid, source).await,
        vec![0],
        "the act must have exactly one execution"
    );
    assert_eq!(
        decided_state(engine, pid, source).await,
        Some(ExecutionState::Completed),
        "the act must hold the completion"
    );
    assert_eq!(
        records_with_prefix(engine, pid, &action).await,
        1,
        "one record for the completion hop"
    );
    assert_eq!(
        records_with_prefix(engine, pid, &propagate).await,
        1,
        "one record for the propagation the completion set off"
    );
}

/// Poll until the run's projection reads the finished flow — one row per model
/// node, every task terminal — or the sweeper has carried the rows away. The
/// rows lag the aggregate's commits and a settled run is never repaired, so both
/// outcomes are the read model converging; a duplicated, stranded or missing row
/// is what the poll fails on. `nodes` is the exact node set the projection must
/// hold (the root included).
async fn wait_rows_finished(engine: &Engine, pid: &str, nodes: &[&str]) {
    let mut expected: Vec<String> = nodes.iter().map(|node| (*node).to_string()).collect();
    expected.sort();
    let what = format!("the rows of {pid} to read its finished one-row-per-node projection");
    poll_until_dumped(
        &what,
        move || {
            let expected = expected.clone();
            async move {
                flush(engine).await;
                match proc_row(engine, pid).await {
                    // a settled run whose rows the sweeper already reclaimed is
                    // the same convergence
                    None => true,
                    Some(proc) => {
                        let mut tids: Vec<String> =
                            proc.tasks().iter().map(|task| task.id.clone()).collect();
                        tids.sort();
                        proc.state().is_biz_success()
                            && tids == expected
                            && proc.tasks().iter().all(|task| task.state().is_completed())
                    }
                }
            }
        },
        || dump_state(engine, pid),
    )
    .await;
}

/// [`wait_rows_finished`] for a fan-out whose exact node set is the container's
/// business, not the cell's: every projected task reads terminal, or the rows
/// are gone.
async fn wait_rows_settled(engine: &Engine, pid: &str) {
    let what = format!("the rows of {pid} to read a settled success");
    poll_until_dumped(
        &what,
        || async {
            flush(engine).await;
            match proc_row(engine, pid).await {
                None => true,
                Some(proc) => {
                    proc.state().is_biz_success()
                        && proc.tasks().iter().all(|task| task.state().is_completed())
                }
            }
        },
        || dump_state(engine, pid),
    )
    .await;
}

// ---------------------------------------------------------------------------
// NEXT × DB失败 — a fault under the scheduled step's own row
// ---------------------------------------------------------------------------

/// A store fault under the *row* of the step a propagation schedules must not
/// refuse, undo or duplicate the work: the row is a projection, so the fault is
/// logged and dropped by the hop that writes it, the client's completion is
/// accepted, and the flow continues into the scheduled step.
///
/// "Exactly once" is the aggregate's: one execution per node (the scheduled
/// step included), the completion's `action:` and `propagate:` records each
/// present exactly once, and the run settles and sweeps cleanly. The read model
/// converges because a later commit of the run re-derives the row it lost; it can
/// never grow a second row for the step.
///
/// Legacy's concern — the task row the propagation creates is lost — maps onto
/// this write: in the legacy engine the row *was* the durable state and the next
/// transition re-persisted it, in the kernel it was always its projection.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_store_fault_under_the_scheduled_steps_row_is_absorbed_and_the_run_finishes_once() {
    bounded(
        "a_store_fault_under_the_scheduled_steps_row_is_absorbed_and_the_run_finishes_once",
        a_store_fault_under_the_scheduled_steps_row_is_absorbed_and_the_run_finishes_once_inner(),
    )
    .await;
}

async fn a_store_fault_under_the_scheduled_steps_row_is_absorbed_and_the_run_finishes_once_inner() {
    let kv = FaultKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, step1) = waiting_head(&engine).await;

    // The fault lands on the row of the step the propagation schedules: the
    // first write of `step2`'s row after arming is the projection of the hop that
    // starts the scheduled step. A row is a projection, so refusing it may not
    // refuse the work it describes.
    let row_key = task_data_key("tasks", &pid, "step2");
    let created = capture_created(&engine, "act2");
    kv.arm_key(&row_key);
    complete_act(&engine, &pid, &step1)
        .await
        .unwrap_or_else(|error| {
            panic!("a row projection fault must not reach the client: {error}")
        });

    // the flow continued into the scheduled step, and the refusal hit the row
    // the cell meant, exactly once
    let (announced, step2) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(
        step2, "step2",
        "the propagation schedules the next model node"
    );
    let armed = kv.clone();
    poll_until(
        &format!("the armed fault on {row_key} to fire"),
        move || {
            let armed = armed.clone();
            async move { armed.fired() == 1 }
        },
    )
    .await;
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(row_key.clone()),
        "the fault must land on the scheduled step's own row"
    );

    // the decision is the aggregate's: the scheduled step waits there, the
    // propagation is whole — one child, one record per hop
    let what = format!("the scheduled step of {pid} to wait in the aggregate");
    poll_until_dumped(
        &what,
        || async { decided_state(&engine, &pid, &step2).await == Some(ExecutionState::Waiting) },
        || dump_state(&engine, &pid),
    )
    .await;
    assert_propagation_once(&engine, &pid, &step1).await;
    assert_eq!(
        generations(&engine, &pid, &step2).await,
        vec![0],
        "the propagation must schedule exactly one child"
    );

    // finish the flow: the client drives the scheduled act
    complete_act(&engine, &pid, &step2).await.unwrap();
    wait_completed(&engine, &pid).await;
    assert_propagation_once(&engine, &pid, &step2).await;
    for node in [utils::consts::TASK_ROOT_TID, step1.as_str(), step2.as_str()] {
        assert_eq!(
            generations(&engine, &pid, node).await,
            vec![0],
            "no node may gain a second execution: {node}"
        );
    }
    assert_eq!(kv.fired(), 1, "the fail-once fault must not re-fire");

    // the read model converges on one row per node — the row the projection
    // lost re-derived by a later commit, or carried away with the run
    wait_rows_finished(
        &engine,
        &pid,
        &[utils::consts::TASK_ROOT_TID, step1.as_str(), step2.as_str()],
    )
    .await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// NEXT × DB失败 — a fault under the propagation's own durable commit
// ---------------------------------------------------------------------------

/// A store that refuses the run's own durable commit — the only place the kernel
/// records a hop — must reach the client instead of being swallowed: the
/// decision was not recorded, so a client told "ok" would believe a propagation
/// nobody holds. The refusal consumes nothing — the act still waits, the step the
/// hop would schedule is still the `Created` execution the model imported, and no
/// `propagate:` record was written — and the crash that follows has nothing of the
/// hop to replay: recovery drives *durable* intents, and this hop left none.
///
/// The propagation is therefore not lost, it is the client's: its retry lands the
/// hop exactly once — one child execution and one record per hop — and no node
/// gains a second generation. Legacy's `next` record family (`ops-id-…`) is the
/// aggregate family now: the kernel keeps the outbox inside the aggregate, so one
/// commit carries the record, and a lost record cannot be re-materialized out of
/// the writer's phase marks.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_store_that_refuses_the_propagations_commit_surfaces_and_the_retry_lands_it_once() {
    bounded(
        "a_store_that_refuses_the_propagations_commit_surfaces_and_the_retry_lands_it_once",
        a_store_that_refuses_the_propagations_commit_surfaces_and_the_retry_lands_it_once_inner(),
    )
    .await;
}

async fn a_store_that_refuses_the_propagations_commit_surfaces_and_the_retry_lands_it_once_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_head(&engine).await;

    // the run's durable truth breaks exactly once: the first commit of the hop
    // the client's completion runs into
    let durable = aggregate_key(&pid);
    kv.arm_key(&durable);
    let refused = complete_act(&engine, &pid, &step1).await;
    match &refused {
        Err(ActError::Store(_)) => {}
        other => panic!("the refused durable commit must reach the client, got {other:?}"),
    }
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(durable),
        "the fault must land on the run's own durable commit"
    );

    // the refusal consumed nothing: the act still waits, the propagation
    // scheduled nothing and recorded nothing
    assert_eq!(
        decided_state(&engine, &pid, &step1).await,
        Some(ExecutionState::Waiting),
        "an act whose propagation was refused still waits for a decision"
    );
    assert_eq!(
        decided_state(&engine, &pid, "step2").await,
        Some(ExecutionState::Created),
        "a refused propagation may not start the step it schedules"
    );
    let (action, propagate) = next_records(&pid, &step1);
    for prefix in [&action, &propagate] {
        assert_eq!(
            records_with_prefix(&engine, &pid, prefix).await,
            0,
            "no record of the refused hop may be durable: {prefix}"
        );
    }
    assert!(
        open_operations(&engine, &pid, &step1).await.is_empty(),
        "the refused hop may not leave a durable intent behind"
    );

    // the crash: close before healing, then boot a fresh engine over the same
    // store. Its recovery pass is the only thing that could replay the hop.
    // (The fault was one-shot: the backend is whole again already.)
    assert!(
        !kv.is_armed(),
        "the armed fault must have been consumed by the refused commit"
    );
    engine.close().await;
    let recovered = engine_on(store.clone()).await;
    let scan = recovered
        .runtime()
        .kernel()
        .recover(SystemClock.now_ms())
        .await
        .unwrap();
    for (what, operations) in [
        ("woken", &scan.woken),
        ("awaited from an owner", &scan.awaiting_owner),
        ("finalized", &scan.finalized),
        ("surfaced for reconciliation", &scan.reconciliation_required),
    ] {
        assert!(
            !operations
                .iter()
                .any(|operation| operation.task_id == step1),
            "the refused hop left nothing to replay; it may not be {what}: {scan:?}"
        );
    }

    // and the recovered run is untouched: the act still waits and no child of
    // the propagation exists
    assert_eq!(
        decided_state(&recovered, &pid, &step1).await,
        Some(ExecutionState::Waiting),
        "recovery may not invent the decision the store refused"
    );
    assert_eq!(
        decided_state(&recovered, &pid, "step2").await,
        Some(ExecutionState::Created),
        "recovery may not start a child of a refused propagation"
    );

    // the client that was told "refused" owns the decision: its retry lands the
    // propagation exactly once
    let created = capture_created(&recovered, "act2");
    complete_act(&recovered, &pid, &step1).await.unwrap();
    let (_, step2) = expect_created(&created).await;
    assert_eq!(step2, "step2", "the retry schedules the next model node");
    wait_waiting_task(&recovered, &pid, &step2).await;
    assert_propagation_once(&recovered, &pid, &step1).await;
    assert_eq!(
        generations(&recovered, &pid, &step2).await,
        vec![0],
        "the retry must schedule exactly one child"
    );

    finish_chain(&recovered, &pid).await;
    wait_rows_finished(
        &recovered,
        &pid,
        &[utils::consts::TASK_ROOT_TID, step1.as_str(), "step2"],
    )
    .await;
    sweep_until_gone(&recovered, &pid).await;
    recovered.close().await;
}

// ---------------------------------------------------------------------------
// NEXT × restart — a propagation left pending is re-driven by recovery
// ---------------------------------------------------------------------------

/// A propagation whose durable intent landed but whose claim never committed (the
/// crash cuts the write right after the intent) is left `Pending` — the state a
/// crash between the two leaves — and the fresh engine's boot recovery is what
/// finishes it: the recovered run reaches its child *without any client action*,
/// the act closes `Completed`, and the hop leaves one child execution and one
/// record per hop, never a second of either.
///
/// Legacy's `Pending` `next` outbox record re-dispatched by
/// `recover_actions` is the kernel's pending `ApplyAction` operation: both are
/// the durable remainder of a decision that was made but not carried out.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_propagation_left_pending_by_a_crash_is_redriven_exactly_once() {
    bounded(
        "a_propagation_left_pending_by_a_crash_is_redriven_exactly_once",
        a_propagation_left_pending_by_a_crash_is_redriven_exactly_once_inner(),
    )
    .await;
}

async fn a_propagation_left_pending_by_a_crash_is_redriven_exactly_once_inner() {
    let kv = CutKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_head(&engine).await;

    // the intent commit lands; the claim that follows it is cut, so the effect
    // never commits
    let before = kv.writes();
    kv.cut_at(before + CLAIM_WRITE);
    let lost = complete_act(&engine, &pid, &step1).await;
    assert!(
        lost.is_err(),
        "a cut writer must reach the client, got {lost:?}"
    );
    assert_eq!(
        kv.refused(),
        1,
        "exactly the write after the intent must have been cut"
    );

    // the crash state: the intent is durable and undecided, the propagation it
    // would have carried out lost
    assert_eq!(
        open_shapes(&open_operations(&engine, &pid, &step1).await),
        vec!["pending"],
        "the crash must leave the propagation's durable intent pending"
    );
    assert_eq!(
        decided_state(&engine, &pid, &step1).await,
        Some(ExecutionState::Waiting),
        "the effect never committed, so the act still waits"
    );
    assert_eq!(
        decided_state(&engine, &pid, "step2").await,
        Some(ExecutionState::Created),
        "the lost effect may not start the step the propagation schedules"
    );
    assert_eq!(
        records_with_prefix(&engine, &pid, &next_records(&pid, &step1).1).await,
        0,
        "the lost effect may not have written the propagation record"
    );

    // the crash and the re-drive: close before healing, then boot a fresh
    // engine over the durable state — its recovery pass re-drives the intent
    engine.close().await;
    kv.heal();
    let recovered = engine_on(store.clone()).await;

    // no client action is taken from here on: only recovery can advance this run
    let what = format!("the recovered propagation of {pid} to land its child once");
    poll_until_dumped(
        &what,
        || async {
            decided_state(&recovered, &pid, &step1).await == Some(ExecutionState::Completed)
                && decided_state(&recovered, &pid, "step2").await == Some(ExecutionState::Waiting)
        },
        || dump_state(&recovered, &pid),
    )
    .await;

    assert_propagation_once(&recovered, &pid, &step1).await;
    assert_eq!(
        generations(&recovered, &pid, "step2").await,
        vec![0],
        "recovery must schedule exactly one child"
    );
    assert!(
        open_operations(&recovered, &pid, &step1).await.is_empty(),
        "the re-driven intent must be settled, not left pending"
    );

    finish_chain(&recovered, &pid).await;
    wait_rows_finished(
        &recovered,
        &pid,
        &[utils::consts::TASK_ROOT_TID, step1.as_str(), "step2"],
    )
    .await;
    sweep_until_gone(&recovered, &pid).await;
    recovered.close().await;
}

// ---------------------------------------------------------------------------
// NEXT × restart — an already-applied propagation is not re-run
// ---------------------------------------------------------------------------

/// An already-applied propagation is never re-run when recovery scans. The
/// kernel commits a hop's effect with its source operation, so an applied
/// propagation is a *closed* operation: the recovery index pages non-terminal
/// operations only, and a fresh boot over the same durable state has nothing of
/// the hop to wake, await, finalize or surface — a repeated scan no more than the
/// first.
///
/// The invariant that legacy's `PropagationPhase` marker protected is therefore
/// structural here: the run keeps one execution per node (the scheduled child
/// included) and one record per hop, and the child is still the very act the
/// crash left waiting — never a second child scheduled over it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn an_already_applied_propagation_is_not_re_run_by_recovery() {
    bounded(
        "an_already_applied_propagation_is_not_re_run_by_recovery",
        an_already_applied_propagation_is_not_re_run_by_recovery_inner(),
    )
    .await;
}

async fn an_already_applied_propagation_is_not_re_run_by_recovery_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the applied propagation is closed: no open operation of the source, one
    // child execution and one record for the hop
    assert!(
        open_operations(&engine, &pid, &step1).await.is_empty(),
        "an applied propagation is a closed operation, not an open record"
    );
    assert_propagation_once(&engine, &pid, &step1).await;
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0],
        "the applied propagation scheduled exactly one child"
    );

    // crash and reload over the same durable state
    let recovered = restart(&engine, store.clone()).await;

    let scan = recovered
        .runtime()
        .kernel()
        .recover(SystemClock.now_ms())
        .await
        .unwrap();
    for (what, operations) in [
        ("woken", &scan.woken),
        ("awaited from an owner", &scan.awaiting_owner),
        ("finalized", &scan.finalized),
        ("surfaced for reconciliation", &scan.reconciliation_required),
    ] {
        assert!(
            !operations
                .iter()
                .any(|operation| operation.task_id == step1),
            "an applied propagation must leave the scan nothing to replay; it may not be {what}: {scan:?}"
        );
    }

    // nothing was re-run: the same single child, the same records, the same
    // waiting act
    assert_propagation_once(&recovered, &pid, &step1).await;
    assert_eq!(
        generations(&recovered, &pid, "step2").await,
        vec![0],
        "recovery may not schedule a second child of the propagation"
    );
    assert_eq!(
        decided_state(&recovered, &pid, "step2").await,
        Some(ExecutionState::Waiting),
        "the recovered run is still where the crash left it"
    );

    // a repeated scan is equally a no-op
    let again = recovered
        .runtime()
        .kernel()
        .recover(SystemClock.now_ms())
        .await
        .unwrap();
    assert!(
        !again
            .woken
            .iter()
            .any(|operation| operation.task_id == step1),
        "the applied propagation may not be re-woken by a repeated scan: {again:?}"
    );

    // and the run finishes exactly once from there
    finish_chain(&recovered, &pid).await;
    for node in [utils::consts::TASK_ROOT_TID, step1.as_str(), "step2"] {
        assert_eq!(
            generations(&recovered, &pid, node).await,
            vec![0],
            "no node may gain a second execution: {node}"
        );
    }
    wait_rows_finished(
        &recovered,
        &pid,
        &[utils::consts::TASK_ROOT_TID, step1.as_str(), "step2"],
    )
    .await;
    sweep_until_gone(&recovered, &pid).await;
    recovered.close().await;
}

// ---------------------------------------------------------------------------
// NEXT × restart — each write boundary of the propagation
// ---------------------------------------------------------------------------

/// Cutting the writer at each of the hop's four writes and booting a fresh engine
/// over the same store must leave the propagation exactly-once or not-at-all, and
/// every boundary must converge on the same single child:
///
/// - the intent write lost: nothing is durable, the scan finds nothing to replay
///   and the act still waits — the client, whose call was refused, owns the
///   decision and its retry schedules the child;
/// - the claim write lost: the intent is durable and `Pending`, and recovery
///   re-drives it without any client action;
/// - the effect write lost: the operation sits at the effect boundary, and the
///   kernel will not guess across it (a refused commit and a lost outcome are
///   indistinguishable there), so an expired claim is reclaimed into
///   `EffectUnknown` and reported for reconciliation; nothing was applied, and
///   the client's retry schedules the child, leaving the ambiguous operation as
///   the only unfinished business;
/// - the close write lost: the effect is applied, recovery finalizes it from its
///   own commit, and the propagation it left pending is woken — the client takes
///   no part.
///
/// In every case the recovered run ends with one execution per node — the source
/// and the child included — and exactly one record per hop, so no boundary can be
/// observed as a second child or a second `propagate:` record.
///
/// The last three writes of the cascade belong to the propagation hop the effect
/// commit seeds, and they are reached from the close boundary: the cut there
/// refuses every write from the close on, so the seeded `PropagateTask` is left
/// whole — `Pending`, with the source operation applied but unclosed — and the
/// healed store's recovery pass drives its claim, effect and close before the
/// client takes any further part.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_at_each_write_boundary_of_the_propagation_converges_on_one_child() {
    bounded(
        "a_crash_at_each_write_boundary_of_the_propagation_converges_on_one_child",
        a_crash_at_each_write_boundary_of_the_propagation_converges_on_one_child_inner(),
    )
    .await;
}

async fn a_crash_at_each_write_boundary_of_the_propagation_converges_on_one_child_inner() {
    for boundary in [INTENT_WRITE, CLAIM_WRITE, EFFECT_WRITE, CLOSE_WRITE] {
        let kv = CutKv::new();
        let store = kv.store();
        let engine = engine_on(store.clone()).await;
        let (pid, step1) = waiting_head(&engine).await;

        // the cut is armed before the action, so the boundary the engine is cut
        // at is the hop's own write and not whichever write the driver made
        // first; nothing else is in flight, so the hop's writes are exactly the
        // writes below the boundary
        let base = kv.writes();
        kv.cut_at(base + boundary);
        let refused = complete_act(&engine, &pid, &step1).await;
        assert!(
            refused.is_err(),
            "boundary {boundary}: a cut writer must reach the client, got {refused:?}"
        );
        assert_eq!(
            kv.refused(),
            1,
            "boundary {boundary}: exactly one write — the hop's own — must have been cut"
        );

        // what the crash left durable, per boundary: the propagation never
        // half-landed
        let open = open_shapes(&open_operations(&engine, &pid, &step1).await);
        match boundary {
            INTENT_WRITE => {
                assert!(
                    open.is_empty(),
                    "boundary 1: the intent never landed, so nothing is left to replay: {open:?}"
                );
                assert_eq!(
                    decided_state(&engine, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "boundary 1: the act still waits for the decision that never landed"
                );
            }
            CLAIM_WRITE => {
                assert_eq!(
                    open,
                    vec!["pending"],
                    "boundary 2: the durable intent must be left undecided for recovery"
                );
                assert_eq!(
                    decided_state(&engine, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "boundary 2: the effect never committed"
                );
            }
            EFFECT_WRITE => {
                assert_eq!(
                    open,
                    vec!["prepared"],
                    "boundary 3: the claim must be left at the effect boundary"
                );
                assert_eq!(
                    decided_state(&engine, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "boundary 3: the engine may not decide across the effect boundary"
                );
            }
            _ => {
                assert_eq!(
                    open,
                    vec!["committed", "pending"],
                    "boundary 4: the effect is applied and the propagation it created pending"
                );
                assert_eq!(
                    decided_state(&engine, &pid, &step1).await,
                    Some(ExecutionState::Completed),
                    "boundary 4: the crash is after the effect, not before it"
                );
            }
        }
        assert_eq!(
            generations(&engine, &pid, &step1).await,
            vec![0],
            "boundary {boundary}: the crash left exactly one execution of the source"
        );
        // The model's executions exist from import; what the propagation owns is
        // *starting* the child, so a hop that lost its effect leaves it
        // un-started.
        if boundary != CLOSE_WRITE {
            assert_eq!(
                decided_state(&engine, &pid, "step2").await,
                Some(ExecutionState::Created),
                "boundary {boundary}: a hop that lost its effect may not start the step it schedules"
            );
        }
        assert_eq!(
            generations(&engine, &pid, "step2").await,
            vec![0],
            "boundary {boundary}: the crash left exactly one execution of the child"
        );
        // The `propagate:` record belongs to the propagation hop the effect
        // commit seeds (it is what moves the successor into `Ready`), so no
        // boundary of this hop can have written it: three of them never seeded
        // the propagation, and at the close boundary the cut refuses every
        // write from the close on — the seeded hop's own commit included.
        assert_eq!(
            records_with_prefix(&engine, &pid, &next_records(&pid, &step1).1).await,
            0,
            "boundary {boundary}: the propagation hop cannot have committed here"
        );

        // heal and boot: the crash state above is the durable remainder
        engine.close().await;
        kv.heal();
        let recovered = engine_on(store.clone()).await;

        // converge. Only the driver of the remainder differs per boundary:
        // recovery for the pending intent and for the applied-but-unclosed hop,
        // the refused client for the two boundaries that lost their decision.
        match boundary {
            INTENT_WRITE => {
                assert_eq!(
                    decided_state(&recovered, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "boundary 1: nothing durable may have decided the act"
                );
                complete_act(&recovered, &pid, &step1).await.unwrap();
            }
            EFFECT_WRITE => {
                let shifted = recovered
                    .runtime()
                    .kernel()
                    .recover(SystemClock.now_ms() + 120_000)
                    .await
                    .unwrap();
                assert!(
                    shifted
                        .reconciliation_required
                        .iter()
                        .any(|operation| operation.task_id == step1),
                    "boundary 3: the ambiguous operation must be surfaced for reconciliation: {shifted:?}"
                );
                assert_eq!(
                    decided_state(&recovered, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "boundary 3: the engine may not decide across the effect boundary"
                );
                complete_act(&recovered, &pid, &step1).await.unwrap();
            }
            _ => {}
        }

        let what = format!("boundary {boundary}: the recovered run {pid} to reach its child once");
        poll_until_dumped(
            &what,
            || async {
                decided_state(&recovered, &pid, &step1).await == Some(ExecutionState::Completed)
                    && decided_state(&recovered, &pid, "step2").await
                        == Some(ExecutionState::Waiting)
            },
            || dump_state(&recovered, &pid),
        )
        .await;
        assert_propagation_once(&recovered, &pid, &step1).await;
        assert_eq!(
            generations(&recovered, &pid, "step2").await,
            vec![0],
            "boundary {boundary}: the propagation must schedule exactly one child"
        );
        if boundary == EFFECT_WRITE {
            assert_eq!(
                open_shapes(&open_operations(&recovered, &pid, &step1).await),
                vec!["effect-unknown"],
                "boundary 3: the ambiguous operation is the only unfinished business left"
            );
        } else {
            assert!(
                open_operations(&recovered, &pid, &step1).await.is_empty(),
                "boundary {boundary}: the driven propagation must be settled, not left open"
            );
        }

        finish_chain(&recovered, &pid).await;
        wait_rows_finished(
            &recovered,
            &pid,
            &[utils::consts::TASK_ROOT_TID, step1.as_str(), "step2"],
        )
        .await;
        sweep_until_gone(&recovered, &pid).await;
        recovered.close().await;
    }
}

// ---------------------------------------------------------------------------
// NEXT × 子任务 — a container waits for the last of its children
// ---------------------------------------------------------------------------

/// A container may not finish before the last child it joins on does. The kernel
/// parks the container's execution `Running` while its children run, and the
/// container join completes it only once every child is terminal — so a step that
/// fans out keeps its own execution non-terminal after its first child settles,
/// and the run is not settled either.
///
/// The branches are told apart through the `Created` message each act dispatches:
/// the read model keeps one row per model node, so two children of one node id
/// could not be distinguished through rows, and each branch here names its own
/// act. Legacy's `PropagationPhase` marker — a parent's `next` staying open while
/// its children ran — is gone; what it protected is the join, pinned here on the
/// aggregate's own state.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_container_completes_only_after_its_last_child_settles() {
    bounded(
        "a_container_completes_only_after_its_last_child_settles",
        a_container_completes_only_after_its_last_child_settles_inner(),
    )
    .await;
}

async fn a_container_completes_only_after_its_last_child_settles_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let left = capture_created(&engine, "act1");
    let right = capture_created(&engine, "act2");
    let pid = start_run(&engine, &branched_workflow()).await;
    let (announced, branch_a) = expect_created(&left).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    let (announced, branch_b) = expect_created(&right).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_ne!(
        branch_a, branch_b,
        "each branch runs its own act node, so the client can name them apart"
    );

    // both branches wait on their own act, and the container is still parked
    // with them
    for tid in [&branch_a, &branch_b] {
        let what = format!("branch act {tid} of {pid} to wait in the aggregate");
        poll_until_dumped(
            &what,
            || async { decided_state(&engine, &pid, tid).await == Some(ExecutionState::Waiting) },
            || dump_state(&engine, &pid),
        )
        .await;
    }
    assert!(
        matches!(
            decided_state(&engine, &pid, "step1").await,
            Some(state) if !state.is_terminal()
        ),
        "the container may not be terminal while its branches run"
    );
    assert!(
        !engine
            .runtime()
            .kernel()
            .process_completed(&pid)
            .await
            .unwrap(),
        "the run may not be settled while a branch runs"
    );

    // one branch settles: the container joins on every child, so it still may
    // not. The window is bounded and only pins the negative — each round also
    // pins the work the join is still waiting for.
    complete_act(&engine, &pid, &branch_a).await.unwrap();
    assert_eq!(
        decided_state(&engine, &pid, &branch_a).await,
        Some(ExecutionState::Completed),
        "the branch the client completed must hold its completion"
    );
    for _ in 0..25 {
        let container = decided_state(&engine, &pid, "step1")
            .await
            .expect("the container's execution");
        assert!(
            !container.is_terminal(),
            "the container finished with a branch still in flight: {container:?}"
        );
        assert!(
            !engine
                .runtime()
                .kernel()
                .process_completed(&pid)
                .await
                .unwrap(),
            "the run may not settle while a branch runs"
        );
        assert_eq!(
            decided_state(&engine, &pid, &branch_b).await,
            Some(ExecutionState::Waiting),
            "the last branch is still the work the join waits for"
        );
        tokio::time::sleep(TICK).await;
    }

    // the last branch settles: now the join may complete the container
    complete_act(&engine, &pid, &branch_b).await.unwrap();
    wait_completed(&engine, &pid).await;
    assert!(
        matches!(
            decided_state(&engine, &pid, "step1").await,
            Some(state) if state.is_terminal()
        ),
        "the container must complete once its last child settled"
    );
    wait_rows_settled(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}
