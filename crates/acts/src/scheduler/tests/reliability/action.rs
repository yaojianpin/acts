//! ACTION reliability cells: the `Next` client action — the wire name of a
//! completion — under a store fault, a duplicate delivery, N racing deliveries,
//! and a crash at each write boundary of its durable hop.
//!
//! The kernel decides a client action in three writes against the aggregate:
//! the durable *intent* (the operation plus the private `__action` fact its hop
//! replays), the claim that prepares it, and the effect commit, which closes
//! the operation inside the same batch. The read model is only projected from
//! those commits afterwards, so a cell reads
//! "exactly once" off the surfaces that hold it — the aggregate's executions
//! and its outbox records (a hop's record key is deterministic per
//! `(pid, tid, generation, sequence)`, so a hop that applied twice shows up as
//! a second record or a second generation) — and "the read model follows" off
//! the projected rows.
//!
//! Three legacy cells pinned the resident scheduler's own bookkeeping: the
//! writer's `PropagationPhase`, the replay of an open `next` record, and the
//! one-shot child-visit marker (`Sign::IN_CHILDREN`). None of it survives the
//! cutover: a decision *is* a committed operation, a hop's effect is committed
//! with its source operation in one guarded batch, and an applied effect leaves
//! no open record behind — so those cells are restated here as the nearest
//! property the kernel holds, and each one says so.

use serial_test::serial;

use crate::scheduler::core::model::clock::{Clock, SystemClock};
use crate::scheduler::core::model::execution_graph::ExecutionState;
use crate::scheduler::core::model::operation::OperationState;
use crate::{ActError, TaskState};

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Start one one-step irq run on `engine`, wait for its act to wait, and answer
/// `(pid, tid)` with every write of the wait settled — the state a client
/// completion is aimed at.
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

/// Start the two-step irq chain and answer `(pid, "step1")` once `step1`'s act
/// waits: the state a crash cell cuts the *first* completion at, where the
/// propagation's child does not exist yet.
async fn waiting_head(engine: &Engine) -> (String, String) {
    let created = capture_created(engine, "act1");
    let pid = start_run(engine, &two_step_irq_workflow()).await;
    let (announced, step1) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    wait_waiting_task(engine, &pid, &step1).await;
    flush(engine).await;
    (pid, step1)
}

/// Start the two-step irq chain, finish `step1`'s act, and answer
/// `(pid, "step1")` once `step2`'s act waits: the smallest run in which a
/// completion has already *propagated* into a fresh child — the state a crash
/// cell needs to see the propagation, its successor and its record.
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
    wait_waiting_task(engine, &pid, &step2).await;
    flush(engine).await;
    (pid, step1)
}

/// Complete whatever of the chain waits, by node, until every execution of the
/// run is terminal: a replay leaves the *next* act waiting, and the client (not
/// the engine) is who completes it.
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

// ---------------------------------------------------------------------------
// The aggregate's own record — where "exactly once" is decided
// ---------------------------------------------------------------------------

/// The run's aggregate document. A settled run keeps it even after its rows are
/// swept, so a cell reads a finished run's decision here and never off a row.
async fn process_snapshot(
    engine: &Engine,
    pid: &str,
) -> crate::scheduler::core::commit::process::ProcessSnapshot {
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
    let mut generations: Vec<u64> = process_snapshot(engine, pid)
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
    process_snapshot(engine, pid)
        .await
        .executions
        .iter()
        .filter(|(id, _)| id.task_id == tid)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, execution)| execution.state)
}

/// The non-terminal operations the aggregate holds for one model node, in
/// sequence order: the durable intent a crash left for recovery to drive.
async fn open_operations(engine: &Engine, pid: &str, tid: &str) -> Vec<OperationState> {
    let mut open: Vec<(u64, OperationState)> = process_snapshot(engine, pid)
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
    process_snapshot(engine, pid)
        .await
        .outbox
        .keys()
        .filter(|id| id.key.starts_with(prefix))
        .count()
}

/// The completion hop's key family for one act: one `action` record for the
/// decision and one `propagate` record for the hop it set off.
fn completion_records(pid: &str, tid: &str) -> (String, String) {
    (
        format!("action:{pid}:{tid}:"),
        format!("propagate:{pid}:{tid}:"),
    )
}

/// The completion landed exactly once: one execution for the act, and exactly
/// one record per hop of the decision.
async fn assert_completion_once(engine: &Engine, pid: &str, tid: &str) {
    let (action_prefix, propagate_prefix) = completion_records(pid, tid);
    assert_eq!(
        generations(engine, pid, tid).await,
        vec![0],
        "the act must have exactly one execution"
    );
    assert_eq!(
        decided_state(engine, pid, tid).await,
        Some(ExecutionState::Completed),
        "the act must hold the completion"
    );
    assert_eq!(
        records_with_prefix(engine, pid, &action_prefix).await,
        1,
        "one record for the completion hop"
    );
    assert_eq!(
        records_with_prefix(engine, pid, &propagate_prefix).await,
        1,
        "one record for the propagation the completion set off"
    );
}

/// The run's projected task rows as a sorted `(tid, state)` list, read after a
/// [`flush`].
async fn task_rows(engine: &Engine, pid: &str) -> Vec<(String, String)> {
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    let mut rows: Vec<(String, String)> = engine
        .runtime()
        .store()
        .tasks()
        .query(&q)
        .await
        .unwrap()
        .rows
        .iter()
        .map(|row| (row.tid.clone(), row.state.clone()))
        .collect();
    rows.sort();
    rows
}

/// Whether `error` is a *decision*-level negative a losing racer may see. There
/// are exactly two families: the refusal the decision guard raises (a `command
/// error:` — `ActionNotAllowed`, which consumes nothing) and the durable step
/// that moved under the racer's own commit (a `commit error:` — a revision
/// conflict, or an operation already past the step it tried to take). A store
/// or engine failure reaching a racer would not be a verdict about the decision
/// at all, and the racing cell treats one as a failure.
fn is_decision_negative(error: &ActError) -> bool {
    match error {
        ActError::Store(_) | ActError::IoError(_) | ActError::QueueFull | ActError::Shutdown => {
            false
        }
        ActError::Action(_) => true,
        ActError::Exception { ecode, .. } => ecode == "ECONFLICT",
        ActError::Runtime(message) => {
            message.contains("command error:")
                || message.contains("commit error:")
                || message.contains("ECONFLICT")
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// ACTION × DB失败 — a fault under the decided act's own row
// ---------------------------------------------------------------------------

/// A store fault under the projection of the completed act's own row must
/// neither fail the client's action nor undo it: the row is derived, so the
/// fault is logged and dropped inside the hop (a projection failure never fails
/// a commit) and the decision stays the aggregate's.
///
/// The act is completed exactly once — one execution and one record per hop —
/// and the read model either converges to the decision (a later full projection
/// of the run re-derives the row it lost: both the action path and the outbox
/// settle pass project every node) or the sweeper carries the stale row away
/// with the run. Either way it never keeps the settled act reading as waiting.
///
/// Legacy's concern — the task-state write the completion triggers is lost —
/// maps onto this write: in the legacy engine the row *was* the durable state,
/// in the kernel it is its projection, and the durable close the legacy writer
/// recovered through is the aggregate commit itself.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_store_fault_under_the_completed_acts_row_keeps_the_decision_and_sweeps() {
    bounded(
        "a_store_fault_under_the_completed_acts_row_keeps_the_decision_and_sweeps",
        a_store_fault_under_the_completed_acts_row_keeps_the_decision_and_sweeps_inner(),
    )
    .await;
}

async fn a_store_fault_under_the_completed_acts_row_keeps_the_decision_and_sweeps_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, tid) = waiting_run(&engine).await;

    // The fault lands on the act's own projected row: exactly the write that
    // would have carried the decision into the read model.
    let row_key = task_data_key("tasks", &pid, &tid);
    kv.arm_key(&row_key);
    complete_act(&engine, &pid, &tid).await.unwrap();
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(row_key),
        "the fault must land on the completed act's own row"
    );

    // the decision is durable: one execution for every node, one record per hop
    wait_completed(&engine, &pid).await;
    flush(&engine).await;
    assert_completion_once(&engine, &pid, &tid).await;

    // one row per node: the fault may not duplicate a row either
    let mut tids: Vec<String> = task_rows(&engine, &pid)
        .await
        .into_iter()
        .map(|(tid, _)| tid)
        .collect();
    tids.sort();
    assert_eq!(
        tids,
        vec![utils::consts::TASK_ROOT_TID.to_string(), tid.clone()],
        "the run's projection must hold exactly one row per node"
    );

    // The read model converges to the decision, or the sweeper carries the
    // stale row away with the run — it never keeps the settled act as waiting.
    let what = format!("the decided act of {pid} to stop reading as waiting");
    poll_until_dumped(
        &what,
        || async {
            match proc_row(&engine, &pid).await {
                Some(proc) => proc
                    .task(&tid)
                    .is_none_or(|task| task.state() != TaskState::Interrupt),
                None => true,
            }
        },
        || dump_state(&engine, &pid),
    )
    .await;

    sweep_until_gone(&engine, &pid).await;

    // A reload observes the finished run: the aggregate keeps the decision and
    // nothing resurrects the act as waiting.
    let reloaded = restart(&engine, store.clone()).await;
    assert_eq!(
        decided_state(&reloaded, &pid, &tid).await,
        Some(ExecutionState::Completed),
        "the aggregate must outlive the swept rows with the act completed"
    );
    if let Some(proc) = proc_row(&reloaded, &pid).await {
        assert!(
            waiting_task(&proc, &tid).is_none(),
            "a reload must never resurrect the completed act as waiting"
        );
    }
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// ACTION × 并发 — N racing completions converge on one decision
// ---------------------------------------------------------------------------

/// N racing `Next` deliveries on the same in-flight act must converge on one
/// decision and one effect: every racer gets a decision-level verdict, at least
/// one reports the decision it applied, the act ends `Completed` exactly once,
/// the run keeps one execution, one row and one record per hop, and a late
/// delivery is refused.
///
/// The kernel does not hold the legacy writer's "exactly one racer succeeds":
/// the intent id is minted per call and the action's retry re-consults the
/// *pinned* intent, so a racer whose intent id collided with the winner's —
/// while carrying the very same decision — reports that decision as its own
/// success, and several racers can legitimately answer `Ok`. They all carry one
/// action here, so every `Ok` is a report of the run's single decision, and
/// what must be exactly once is the durable effect: one execution, one row and
/// one record per hop, checked on the aggregate. A losing racer's negative is
/// never an engine failure — only a refusal or a durable step that moved under
/// it — and the run always reaches the decision it raced for.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn racing_completes_converge_on_one_decision_and_one_effect() {
    bounded(
        "racing_completes_converge_on_one_decision_and_one_effect",
        racing_completes_converge_on_one_decision_and_one_effect_inner(),
    )
    .await;
}

async fn racing_completes_converge_on_one_decision_and_one_effect_inner() {
    const RACERS: usize = 8;
    for round in 0..3 {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, tid) = waiting_run(&engine).await;

        let rt = engine.runtime();
        let mut racers = tokio::task::JoinSet::new();
        for _ in 0..RACERS {
            let rt = rt.clone();
            let (pid, tid) = (pid.clone(), tid.clone());
            racers.spawn(async move {
                rt.do_action(&Action::new(&pid, &tid, EventAction::Next, Vars::new()))
                    .await
            });
        }
        let mut verdicts = Vec::with_capacity(RACERS);
        while let Some(joined) = racers.join_next().await {
            verdicts.push(joined.expect("no racer may panic"));
        }

        assert_eq!(
            verdicts.len(),
            RACERS,
            "round {round}: every racer must get a verdict"
        );
        assert!(
            verdicts.iter().any(|verdict| verdict.is_ok()),
            "round {round}: the decision that landed must be reported by a racer: {:?}",
            verdicts.iter().map(outcome).collect::<Vec<_>>()
        );
        for verdict in &verdicts {
            if let Err(error) = verdict {
                assert!(
                    is_decision_negative(error),
                    "round {round}: a losing racer must be refused or lose a durable step, got {error:?}"
                );
            }
        }

        // the decision the race converged on, on the durable surface
        wait_completed(&engine, &pid).await;
        flush(&engine).await;
        assert_completion_once(&engine, &pid, &tid).await;
        let unfinished: Vec<String> = read_proc(&engine, &pid)
            .await
            .tasks()
            .iter()
            .filter(|task| !task.state().is_completed())
            .map(|task| format!("{}:{}", task.id, task.state()))
            .collect();
        assert!(
            unfinished.is_empty(),
            "round {round}: no task may be left non-terminal: {unfinished:?}"
        );
        let mut tids: Vec<String> = task_rows(&engine, &pid)
            .await
            .into_iter()
            .map(|(tid, _)| tid)
            .collect();
        tids.sort();
        assert_eq!(
            tids,
            vec![utils::consts::TASK_ROOT_TID.to_string(), tid.clone()],
            "round {round}: one row per node, no duplicate from a racer"
        );

        // the race's decision is final: a late completion is refused
        assert!(
            complete_act(&engine, &pid, &tid).await.is_err(),
            "round {round}: a late completion on the decided act must be refused"
        );
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}

// ---------------------------------------------------------------------------
// ACTION × 重复投递 — the duplicate is refused, not re-applied
// ---------------------------------------------------------------------------

/// Delivering the same completion twice must decide the act exactly once: the
/// second delivery is refused by the decision guard (a `Completed` execution
/// has no `Complete` target) and consumes nothing at all — the guard runs in
/// the preflight, before the durable intent, so the aggregate is untouched.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_complete_is_refused_without_effect() {
    bounded(
        "a_duplicate_complete_is_refused_without_effect",
        a_duplicate_complete_is_refused_without_effect_inner(),
    )
    .await;
}

async fn a_duplicate_complete_is_refused_without_effect_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, tid) = waiting_run(&engine).await;

    complete_act(&engine, &pid, &tid).await.unwrap();
    // let the first decision settle durably before the duplicate arrives, so
    // the duplicate is a retry and not a race with the first delivery
    wait_completed(&engine, &pid).await;
    flush(&engine).await;
    let before = process_snapshot(&engine, &pid).await;
    let before_rows = task_rows(&engine, &pid).await;

    let duplicate = complete_act(&engine, &pid, &tid).await;
    assert!(
        duplicate.is_err(),
        "a duplicate completion must never be applied again, got {duplicate:?}"
    );

    // the refusal consumed nothing: same operations, same executions, same
    // records, same rows
    flush(&engine).await;
    let after = process_snapshot(&engine, &pid).await;
    assert_eq!(
        after.operations.len(),
        before.operations.len(),
        "a refused duplicate may not create an operation"
    );
    assert_eq!(
        after.executions.len(),
        before.executions.len(),
        "a refused duplicate may not move an execution"
    );
    let mut before_keys: Vec<&String> = before.outbox.keys().map(|id| &id.key).collect();
    let mut after_keys: Vec<&String> = after.outbox.keys().map(|id| &id.key).collect();
    before_keys.sort();
    after_keys.sort();
    assert_eq!(
        before_keys, after_keys,
        "a refused duplicate may not add an outbox record"
    );
    // A settled run's rows may be reclaimed by the sweeper at any later tick,
    // so the projection may only *lose* rows; it can never change one or grow.
    let after_rows = task_rows(&engine, &pid).await;
    assert!(
        after_rows.len() <= before_rows.len()
            && after_rows.iter().all(|row| before_rows.contains(row)),
        "a refused duplicate may not change or add a projected row: {before_rows:?} -> {after_rows:?}"
    );
    assert_eq!(
        decided_state(&engine, &pid, &tid).await,
        Some(ExecutionState::Completed),
        "the act must keep the first decision"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ACTION × restart — each write boundary of the completion
// ---------------------------------------------------------------------------

/// The completion hop writes the aggregate three times: the durable *intent*
/// (the operation plus the private action fact), the claim that prepares it,
/// and the effect commit (which closes the operation). Cutting the writer at
/// each of the first three and then booting a fresh engine over the same store
/// must leave the client's decision exactly-once or not-at-all — never applied
/// twice, never silently dropped:
///
/// - the intent write lost: nothing is durable, the recovery scan finds nothing
///   to replay (`scanned == 0`) and the act still waits for the client, which
///   its own refusal already told;
/// - the claim write lost: the intent is durable and `Pending`, and recovery
///   re-drives it exactly once — the decision lands, one record per hop, the
///   chain propagates into one child;
/// - the effect write lost: the operation sits at the effect boundary, and the
///   kernel will not guess across it (a refused commit and a lost outcome are
///   indistinguishable there), so an expired claim is reclaimed into
///   `EffectUnknown` and *reported for reconciliation* instead of being
///   re-applied; nothing is applied and the act still waits — the client, whose
///   call was refused, owns the reconciliation, and its retry decides the act
///   exactly once.
///
/// Legacy split the same window with the writer's `PropagationPhase`; the
/// kernel's equivalent is the operation's own state machine, and its effect
/// boundary is deliberately fail-safe rather than replaying an effect it cannot
/// prove was not applied.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_at_each_write_boundary_of_the_completion_keeps_the_decision_once() {
    bounded(
        "a_crash_at_each_write_boundary_of_the_completion_keeps_the_decision_once",
        a_crash_at_each_write_boundary_of_the_completion_keeps_the_decision_once_inner(),
    )
    .await;
}

async fn a_crash_at_each_write_boundary_of_the_completion_keeps_the_decision_once_inner() {
    const INTENT: usize = 1;
    const CLAIM: usize = 2;
    const EFFECT: usize = 3;

    for boundary in [INTENT, CLAIM, EFFECT] {
        let kv = CutKv::new();
        let store = kv.store();
        let engine = engine_on(store.clone()).await;
        let (pid, step1) = waiting_head(&engine).await;

        // The cut is armed *before* the action, so the boundary the engine is
        // cut at is the hop's own write and not whichever write the driver made
        // first; nothing else is in flight, so the hop's writes are exactly the
        // writes below the boundary.
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

        // what the crash left durable, per boundary
        let open = open_shapes(&open_operations(&engine, &pid, &step1).await);
        match boundary {
            INTENT => assert!(
                open.is_empty(),
                "boundary 1: the intent never landed, so nothing is left to replay: {open:?}"
            ),
            CLAIM => assert_eq!(
                open,
                vec!["pending"],
                "boundary 2: the durable intent must be left undecided for recovery"
            ),
            _ => assert_eq!(
                open,
                vec!["prepared"],
                "boundary 3: the claim must be left at the effect boundary"
            ),
        }

        engine.close().await;
        kv.heal();
        let recovered = engine_on(store.clone()).await;
        let scan = recovered
            .runtime()
            .kernel()
            .recover(SystemClock.now_ms())
            .await
            .unwrap();

        match boundary {
            INTENT => {
                assert_eq!(
                    scan.scanned, 0,
                    "nothing durable may be left to replay: {scan:?}"
                );
                assert_eq!(
                    decided_state(&recovered, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "the act must still wait for a decision that never landed"
                );
                assert_eq!(
                    records_with_prefix(&recovered, &pid, &completion_records(&pid, &step1).0)
                        .await,
                    0,
                    "nothing of the refused hop may be durable"
                );
            }
            CLAIM => {
                // recovery re-drives the pending intent exactly once, and the
                // chain advances into the next act
                let what = format!("boundary {boundary}: the act of {pid} to be decided once");
                let advanced = poll_until_dumped(
                    &what,
                    || async {
                        decided_state(&recovered, &pid, &step1).await
                            == Some(ExecutionState::Completed)
                            && decided_state(&recovered, &pid, "step2").await
                                == Some(ExecutionState::Waiting)
                    },
                    || dump_state(&recovered, &pid),
                )
                .await;
                assert!(
                    advanced,
                    "boundary {boundary}: the replayed decision must land"
                );
                assert_completion_once(&recovered, &pid, &step1).await;
            }
            _ => {
                // The effect boundary is reclaimed into `EffectUnknown` and
                // surfaced for reconciliation; nothing is applied.
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
                    "the effect boundary must be surfaced for reconciliation: {shifted:?}"
                );
                assert_eq!(
                    decided_state(&recovered, &pid, &step1).await,
                    Some(ExecutionState::Waiting),
                    "the engine may not decide across the effect boundary"
                );
                assert_eq!(
                    records_with_prefix(&recovered, &pid, &completion_records(&pid, &step1).0)
                        .await,
                    0,
                    "no effect may be committed from the ambiguous boundary"
                );
                // the client owns the reconciliation: its retry decides once
                complete_act(&recovered, &pid, &step1).await.unwrap();
                let what = format!("boundary {boundary}: the retried decision to land once");
                let advanced = poll_until_dumped(
                    &what,
                    || async {
                        decided_state(&recovered, &pid, &step1).await
                            == Some(ExecutionState::Completed)
                            && decided_state(&recovered, &pid, "step2").await
                                == Some(ExecutionState::Waiting)
                    },
                    || dump_state(&recovered, &pid),
                )
                .await;
                assert!(
                    advanced,
                    "boundary {boundary}: the retry must decide the act"
                );
                assert_completion_once(&recovered, &pid, &step1).await;
                assert_eq!(
                    open_shapes(&open_operations(&recovered, &pid, &step1).await),
                    vec!["effect-unknown"],
                    "the ambiguous operation is the only unfinished business left"
                );
            }
        }

        // no boundary may duplicate a node of the run
        assert_eq!(generations(&recovered, &pid, &step1).await, vec![0]);
        assert_eq!(generations(&recovered, &pid, "step2").await, vec![0]);

        if boundary != INTENT {
            finish_chain(&recovered, &pid).await;
            sweep_until_gone(&recovered, &pid).await;
        }
        recovered.close().await;
    }
}

// ---------------------------------------------------------------------------
// ACTION × restart — a crash after the chain propagated keeps one child
// ---------------------------------------------------------------------------

/// The crash legacy called "one child without the visit marker": the chain has
/// already propagated into `step2`, the parent's propagation is applied, and
/// the crash carries it away.
///
/// The legacy engine kept the parent's `next` record `Pending` for as long as
/// its children were in flight and replayed it through recovery, relying on a
/// one-shot in-memory child-visit marker to keep the replay from scheduling a
/// second child. The kernel has neither: a hop's effect is committed with its
/// source operation, so an applied propagation is a *closed* operation in the
/// aggregate — nothing is left `Pending` for the scan to replay, the child's
/// execution is already there, and the recovery scan is asserted to have
/// nothing to wake, await, finalize or surface for it. The cell pins that on
/// the surface the kernel has — a crash taken at the next act's completion
/// intent, with the chain already propagated — and checks that the recovered
/// run keeps exactly one execution per node, one record for the applied
/// propagation, and re-applies only the client's decision, exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_after_the_chain_propagated_keeps_one_child_per_node() {
    bounded(
        "a_crash_after_the_chain_propagated_keeps_one_child_per_node",
        a_crash_after_the_chain_propagated_keeps_one_child_per_node_inner(),
    )
    .await;
}

async fn a_crash_after_the_chain_propagated_keeps_one_child_per_node_inner() {
    // The completion hop's first two writes: the intent and the claim. Cutting
    // at the claim leaves the intent — the durable remainder — landed.
    const INTENT_LANDS_AT_CLAIM: usize = 2;

    let kv = CutKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the applied propagation leaves nothing open: its operation is closed and
    // the child it created is the only execution of `step2` there is
    assert!(
        open_operations(&engine, &pid, &step1).await.is_empty(),
        "an applied propagation must be a closed operation, not an open record"
    );
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0],
        "the propagation created exactly one child"
    );
    assert_eq!(
        records_with_prefix(&engine, &pid, &completion_records(&pid, &step1).1).await,
        1,
        "the applied propagation owns exactly one record"
    );

    // the crash: cut before the second act's completion, so its intent is the
    // durable remainder the fresh engine resumes from
    let base = kv.writes();
    kv.cut_at(base + INTENT_LANDS_AT_CLAIM);
    let refused = complete_act(&engine, &pid, "step2").await;
    assert!(
        refused.is_err(),
        "a cut writer must reach the client, got {refused:?}"
    );
    assert_eq!(
        kv.refused(),
        1,
        "exactly the second act's completion intent must have been cut"
    );
    assert_eq!(
        open_shapes(&open_operations(&engine, &pid, "step2").await),
        vec!["pending"],
        "the crash must leave the second act's intent undecided"
    );

    engine.close().await;
    kv.heal();
    let recovered = engine_on(store.clone()).await;

    // The recovery scan has nothing of the applied propagation to replay: none
    // of its source node's operations is woken, awaited, finalized or surfaced
    // for reconciliation — only the client's decision is left to re-derive.
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
            "the applied propagation must leave the scan nothing to replay; it may not be {what}: {scan:?}"
        );
    }

    // recovery re-drives the decision and the run finishes once
    wait_completed(&recovered, &pid).await;

    // one child per node: the recovered run created no second `step1`/`step2`,
    // and the applied propagation stayed a single closed record
    for node in [step1.as_str(), "step2", utils::consts::TASK_ROOT_TID] {
        assert_eq!(
            generations(&recovered, &pid, node).await,
            vec![0],
            "a crash after the propagation may not add a second execution of {node}"
        );
    }
    assert_completion_once(&recovered, &pid, "step2").await;
    assert_eq!(
        records_with_prefix(&recovered, &pid, &completion_records(&pid, &step1).1).await,
        1,
        "the applied propagation must stay one record"
    );
    assert!(
        open_operations(&recovered, &pid, "step2").await.is_empty(),
        "the re-driven decision must be closed"
    );

    sweep_until_gone(&recovered, &pid).await;
    recovered.close().await;
}
