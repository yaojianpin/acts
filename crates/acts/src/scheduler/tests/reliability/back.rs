//! BACK reliability cells: the `Back` client action — the rewind that closes
//! the waiting act's generation `Backed` and re-enters the step the client
//! named with a fresh generation — under a crash between its intent and its
//! effect, a store fault on the rewind's first durable write, a duplicate
//! delivery, and a race with `Next`.
//!
//! The kernel decides a client action by committing against the aggregate and
//! projects the rows from the committed snapshot afterwards. Its `Back` is the
//! legacy redo: it needs a *waiting* execution and the `to` step to re-enter,
//! and its deciding commit closes the source generation `Backed`, pins the
//! redo target and creates the durable `ReenterTask` operation whose operation
//! rebuilds the named step and its forward closure as fresh generations. So
//! the cells pin, on the surfaces the kernel has:
//!
//! - "exactly one rewind" is a *generation* fact — the aggregate's generations
//!   for each model node, plus the node's own `Created` message — never a task
//!   count (the read model keeps one row per model node, so a count cannot see
//!   a rewind at all);
//! - a durable intent a crash left behind is replayed by recovery exactly
//!   once (an aggregate a writer from before the operation's close was folded into
//!   its effect commit left `Committed` is finalized rather than re-applied —
//!   that state is pinned by `async_document_store.rs`'s finalize cell, since
//!   no operation produces it any more);
//! - a store fault the rewind's own durable commit runs into is surfaced to
//!   the client and consumes nothing, so the retry rewinds exactly once;
//! - a duplicate `Back` is refused by the decision guard — after the rewind
//!   the newest generation of the node it names is the redo's own forward act,
//!   *created* but not yet run, and the guard only admits a waiting execution
//!   — so a second delivery can add no second rewind;
//! - a `Back` racing the `Next` that finishes the same waiting act converges
//!   on one decision, with a rewind iff the back won.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serial_test::serial;

use crate::ActError;
use crate::scheduler::core::model::operation::OperationKind;
use crate::scheduler::core::model::task_graph::TaskState;

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Start a two-step irq run, finish `step1`'s act, and wait until `step2`'s
/// act waits: the state a `Back` is aimed at — the finished head step is the
/// client's `to`, the waiting act is the node the client names.
///
/// The head act's `Created` is consumed here; a cell that wants the *redo's*
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
        step1,
        tid_of(&pid, "step1", 0),
        "the act of a `uses` step is the step's own instance"
    );
    wait_waiting_task(engine, &pid, "step1").await;

    let created = capture_created(engine, "act2");
    complete_act(engine, &pid, &step1).await.unwrap();
    let (announced, step2) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(
        step2,
        tid_of(&pid, "step2", 0),
        "the second act is the second step's own instance"
    );
    wait_waiting_task(engine, &pid, "step2").await;
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
    // ticks and the completion's operations can be starved well past the stand-alone
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
                // The waiting act may be a rewind's redo (generation 1), so
                // both generations get the completion: the terminal one
                // refuses it, the live one takes it.
                for generation in 0..=1 {
                    let _ = complete_act(engine, pid, &tid_of(pid, node, generation)).await;
                }
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

/// The aggregate's state for one generation of one node.
async fn generation_state(
    engine: &Engine,
    pid: &str,
    tid: &str,
    generation: u64,
) -> Option<TaskState> {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    let node = super::node_of(&snapshot, pid, tid);
    snapshot
        .tasks
        .iter()
        .find(|(id, _)| id.node.as_str() == node && id.generation == generation)
        .map(|(_, task)| task.state)
}

/// Whether the aggregate still holds a non-terminal operation for `tid`: the
/// durable work a crash left for recovery to drive.
async fn has_unsettled_operation(engine: &Engine, pid: &str, tid: &str) -> bool {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    let node = super::node_of(&snapshot, pid, tid);
    snapshot
        .operations
        .values()
        .any(|operation| operation.id.task.node.as_str() == node && !operation.is_terminal())
}

/// Whether the aggregate still holds a pure-internal hand-off: the
/// `PropagateTask`/`SkipTask`/`AbortTask`/`ReenterTask` operations A3 folds
/// into the commit that produced them. A rewind's fresh generations are the
/// durable proof; a surviving hand-off would be a second durable step.
async fn pure_operations(engine: &Engine, pid: &str) -> usize {
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
        .filter(|operation| {
            matches!(
                operation.kind,
                OperationKind::PropagateTask
                    | OperationKind::SkipTask
                    | OperationKind::AbortTask
                    | OperationKind::ReenterTask
            )
        })
        .count()
}

/// The crash and the boot, in the order that makes the crash real: close
/// `engine` while the cut still refuses every write, so no live driver can
/// apply the operation the crash left behind, then heal the store (an engine boot
/// writes, so the store must be whole before `start`) and boot a fresh engine
/// over the same durable state. This is the harness's [`restart`] with the
/// healing sequenced between the close and the boot.
async fn crash_and_reboot(engine: &Engine, store: Arc<dyn KvStore>, kv: &CutKv) -> Engine {
    engine.close().await;
    kv.heal();
    engine_on(store).await
}

// ---------------------------------------------------------------------------
// BACK × restart — a crash between the intent and the effect
// ---------------------------------------------------------------------------

/// A `Back` whose durable intent landed but whose effect never committed (a
/// crash between the two, here a write cut) must be replayed by recovery
/// exactly once: the intent is the durable decision — the action the client
/// made — and the operation that follows it re-plans it, closing the waiting act's
/// generation `Backed` and creating the `ReenterTask` rewind, so the rewind
/// survives the crash instead of being silently dropped.
///
/// The cut is placed at the back's second durable write (the claim right after
/// the intent), so the intent is durable and `Pending` while the effect is
/// lost: exactly the state a crash between the two leaves. The legacy concern
/// applies unchanged — a recovery that closed the intent because the target
/// act is no longer waiting would drop the rewind the client was promised —
/// and this cell waits for the rewind to *run*: the redone act waits on the
/// client and its second `Created` is the announcement, so the property is the
/// rewind running once rather than merely a generation appearing.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_back_whose_intent_landed_is_reapplied_exactly_once_on_recovery() {
    bounded(
        "sch_back_whose_intent_landed_is_reapplied_exactly_once_on_recovery",
        a_back_whose_intent_landed_is_reapplied_exactly_once_on_recovery_inner(),
    )
    .await;
}

async fn a_back_whose_intent_landed_is_reapplied_exactly_once_on_recovery_inner() {
    let kv = CutKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the intent commit lands; the claim that follows it is cut, so the effect
    // never commits and the source act keeps waiting
    let before = kv.writes();
    kv.cut_at(before + 2);
    let lost = back_act(&engine, &pid, &tid_of(&pid, "step2", 0), "step1").await;
    assert!(
        lost.is_err(),
        "a cut writer must reach the client, got {lost:?}"
    );
    assert!(
        kv.refused() >= 1,
        "the cut must have refused the write after the intent"
    );

    // the crash state: the intent is durable and undecided, its effect lost
    assert!(
        has_unsettled_operation(&engine, &pid, "step2").await,
        "the crash must leave the back's durable intent pending"
    );
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "the lost effect must not have rewound"
    );
    assert_eq!(
        generation_state(&engine, &pid, "step2", 0).await,
        Some(TaskState::Waiting),
        "the source act is still waiting, the state the replayed back reads"
    );

    // the crash and the replay: the fresh engine's recovery pass is what
    // re-drives the intent, exactly once. The redo's own `Created` is
    // registered first: the re-enter commits on the fresh engine's operation, and
    // the act's message rides the outbox pass after it, so the announcement is
    // observed rather than missed.
    let reloaded = crash_and_reboot(&engine, store.clone(), &kv).await;
    let rewind = capture_created(&reloaded, "act1");

    let what = format!("the recovered back of {pid} to rewind the named step once");
    poll_until_dumped(
        &what,
        || async { generations(&reloaded, &pid, &step1).await == vec![0, 1] },
        || async {
            format!(
                "{} | head gens={:?} source={:?} intent-unsettled={}",
                dump_state(&reloaded, &pid).await,
                generations(&reloaded, &pid, &step1).await,
                generation_state(&reloaded, &pid, "step2", 0).await,
                has_unsettled_operation(&reloaded, &pid, "step2").await,
            )
        },
    )
    .await;
    // the redone act is dispatched to the client exactly once: it waits, and
    // its announcement is the node's second `Created`
    wait_waiting_task(&reloaded, &pid, "step1").await;
    let (announced, redone) = expect_created(&rewind).await;
    assert_eq!(announced, pid, "the replay names its own run");
    assert_eq!(
        redone,
        tid_of(&pid, "step1", 1),
        "the replay re-enters the node the client named"
    );

    assert_eq!(
        generations(&reloaded, &pid, &step1).await,
        vec![0, 1],
        "recovery must re-apply the back exactly once"
    );
    assert_eq!(
        generations(&reloaded, &pid, "step2").await,
        vec![0, 1],
        "the rewind's forward chain must be recreated exactly once"
    );
    assert_eq!(
        generation_state(&reloaded, &pid, "step2", 0).await,
        Some(TaskState::Backed),
        "the replayed back must close the waiting source generation Backed"
    );
    assert_eq!(
        pure_operations(&reloaded, &pid).await,
        0,
        "the rewind is folded into the deciding commit, never a second durable hand-off"
    );
    assert!(
        !has_unsettled_operation(&reloaded, &pid, "step2").await,
        "the replayed intent must be completed, not left pending"
    );

    finish_by_hand(&reloaded, &pid).await;
    assert_settled(&reloaded, &pid).await;
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// BACK × DB失败 — the rewind's own durable step breaks once
// ---------------------------------------------------------------------------

/// A store fault under the back's own durable commit must reach the client
/// instead of being swallowed: that commit *is* the durable decision, so a
/// client told "ok" would have lost a rewind it was promised. The refusal
/// consumes nothing — the source act is still waiting, no generation was
/// re-entered — and the retry (the fault is one-shot) rewinds exactly once, so
/// the run settles and sweeps with one redo and no duplicated work.
///
/// The kernel has no per-act durable write, and it creates the rewind inside
/// the very commit that carries the back: where the legacy cell broke the
/// first write of the redo work the action created, the nearest durable unit
/// is the run's aggregate commit — the write the rewind path persists first,
/// since the rewind *is* that commit's child operation. Breaking it is
/// therefore breaking the rewind's first durable step, and the cell proves
/// both halves: the fault fired once on that document, and the one-shot
/// failure healed without rewinding twice.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_back_store_fault_at_the_backs_durable_step_surfaces_and_heals_once() {
    bounded(
        "sch_back_store_fault_at_the_backs_durable_step_surfaces_and_heals_once",
        a_store_fault_at_the_backs_durable_step_surfaces_and_heals_once_inner(),
    )
    .await;
}

async fn a_store_fault_at_the_backs_durable_step_surfaces_and_heals_once_inner() {
    let kv = FaultKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the one-shot fault is aimed at the run's aggregate document: the back's
    // commit (and the operation that carries it) is the next write of it
    let process = process_key(&pid);
    kv.arm_key(&process);
    let refused = back_act(&engine, &pid, &tid_of(&pid, "step2", 0), "step1").await;
    match &refused {
        Err(ActError::Store(_)) => {}
        other => panic!("the refused durable commit must reach the client, got {other:?}"),
    }
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(process),
        "the fault must land on the run's aggregate commit"
    );

    // the refusal consumed nothing: the source act still waits, nothing was
    // re-entered, and no rewind operation was created
    assert!(
        waiting_task(&read_proc(&engine, &pid).await, "step2").is_some(),
        "a refused back must leave the act waiting"
    );
    assert_eq!(
        generation_state(&engine, &pid, "step2", 0).await,
        Some(TaskState::Waiting)
    );
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0],
        "a refused back must not rewind"
    );
    assert_eq!(
        pure_operations(&engine, &pid).await,
        0,
        "a refused back must not create a folded hand-off"
    );

    // the healed retry lands one rewind, observed on the redo's own `Created`
    let rewind = capture_created(&engine, "act1");
    back_act(&engine, &pid, &tid_of(&pid, "step2", 0), "step1")
        .await
        .unwrap();
    assert_eq!(kv.fired(), 1, "the one-shot fault must not fire twice");
    let (_, redone) = expect_created(&rewind).await;
    assert_eq!(
        redone,
        tid_of(&pid, "step1", 1),
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
        "the rewind's forward chain must be recreated exactly once"
    );
    assert_eq!(
        pure_operations(&engine, &pid).await,
        0,
        "the rewind is folded into the healed deciding commit"
    );

    finish_by_hand(&engine, &pid).await;
    assert_settled(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// BACK × 重复投递 — the duplicate is refused, not re-applied
// ---------------------------------------------------------------------------

/// Delivering the same `Back` twice must rewind exactly once: the first
/// delivery closes the waiting act's generation `Backed` and re-enters the
/// named step — one fresh generation for the target, one fresh act `Created` —
/// and the second delivery is refused by the decision guard, because the
/// newest generation of the node it names is now the redo's own forward act,
/// *created* but not yet run, and the guard only admits a `Waiting` execution.
/// The refused duplicate consumes nothing: no second rewind operation, no
/// second generation, no second redo act — and the run still finishes through
/// the one rewind the client drove.
///
/// The legacy duplicate was rejected because its target *task* was terminal;
/// the kernel resolves a client action to the node's newest generation, so the
/// refusal is a fact about that generation — the redo's forward act, still
/// live — rather than about a task identity the kernel does not keep.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_back_duplicate_back_is_refused_without_a_second_rewind() {
    bounded(
        "sch_back_duplicate_back_is_refused_without_a_second_rewind",
        a_duplicate_back_is_refused_without_a_second_rewind_inner(),
    )
    .await;
}

async fn a_duplicate_back_is_refused_without_a_second_rewind_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, step1) = waiting_chain(&engine).await;

    // the rewind's own `Created`, registered before the back that asks for it
    let rewind = capture_created(&engine, "act1");
    back_act(&engine, &pid, &tid_of(&pid, "step2", 0), "step1")
        .await
        .unwrap();
    let (announced, redone) = expect_created(&rewind).await;
    assert_eq!(announced, pid, "the rewind names its own run");
    assert_eq!(
        redone,
        tid_of(&pid, "step1", 1),
        "the rewind re-enters the node the client named"
    );
    wait_waiting_task(&engine, &pid, "step1").await;

    // the redo's forward act exists but has not run: this is what the
    // duplicate will be resolved against
    assert_eq!(
        generation_state(&engine, &pid, "step2", 1).await,
        Some(TaskState::Created),
        "the rewind's forward act must exist and not yet be running"
    );

    // every later `Created` of the node is observed: the duplicate must not
    // announce one
    let extra = capture_created(&engine, "act1");
    let duplicate = back_act(&engine, &pid, &tid_of(&pid, "step2", 0), "step1").await;
    assert!(
        duplicate.is_err(),
        "a duplicate back must be refused, got {duplicate:?}"
    );
    let announced = tokio::time::timeout(Duration::from_millis(2000), extra.recv()).await;
    assert!(
        announced.is_err(),
        "the refused duplicate must not announce a second rewind: {:?}",
        announced.ok()
    );

    // exactly one rewind: one fresh generation for the target and one for the
    // forward chain the rewind closed over, and one durable rewind operation
    assert_eq!(
        generations(&engine, &pid, &step1).await,
        vec![0, 1],
        "the named step must be redone exactly once"
    );
    assert_eq!(
        generations(&engine, &pid, "step2").await,
        vec![0, 1],
        "the forward chain must be recreated exactly once"
    );
    assert_eq!(
        pure_operations(&engine, &pid).await,
        0,
        "the duplicate may not add a second concrete rewind"
    );

    finish_by_hand(&engine, &pid).await;
    assert_settled(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// BACK × 并发 — Back racing the Next that finishes the same act
// ---------------------------------------------------------------------------

/// `Back` and `Next` fired at the same waiting act in the same instant must
/// converge on exactly one durable decision: `Next` decides the waiting act
/// `Completed` and the rewind re-enters the named step iff the `Back` won —
/// the raced generation holds exactly one terminal state, the rewind exists
/// exactly once iff the back won (a `Next` win must leave no orphan redo
/// work), and the run settles in one outcome either way. Several rounds cover
/// the interleavings.
///
/// The decision is durable, not just in-memory: the losing racer must be
/// refused by the guard, because an admitted pair would leave the forward path
/// completed *and* a live rewind under it — duplicate execution plus orphan
/// work.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_back_racing_complete_converges_on_one_terminal_decision() {
    bounded(
        "sch_back_racing_complete_converges_on_one_terminal_decision",
        back_racing_complete_converges_on_one_terminal_decision_inner(),
    )
    .await;
}

async fn back_racing_complete_converges_on_one_terminal_decision_inner() {
    for round in 0..3 {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, step1) = waiting_chain(&engine).await;

        // the race: both deliveries target the same waiting act at once
        let rewind = capture_created(&engine, "act1");
        let step2 = tid_of(&pid, "step2", 0);
        let (back, next) = tokio::join!(
            back_act(&engine, &pid, &step2, "step1"),
            complete_act(&engine, &pid, &step2),
        );
        assert!(
            back.is_ok() ^ next.is_ok(),
            "round {round}: exactly one racer may decide the act, got back={} next={}",
            outcome(&back),
            outcome(&next)
        );

        let rewound = back.is_ok();
        if rewound {
            let (_, tid) = expect_created(&rewind).await;
            assert_eq!(
                tid,
                tid_of(&pid, "step1", 1),
                "round {round}: a winning back re-enters the named step"
            );
        } else {
            let announced = tokio::time::timeout(Duration::from_millis(2000), rewind.recv()).await;
            assert!(
                announced.is_err(),
                "round {round}: a refused back must not rewind: {:?}",
                announced.ok()
            );
        }

        assert_eq!(
            generation_state(&engine, &pid, "step2", 0).await,
            Some(if rewound {
                TaskState::Backed
            } else {
                TaskState::Completed
            }),
            "round {round}: the raced act must hold exactly one terminal decision"
        );
        let expected = if rewound { vec![0, 1] } else { vec![0] };
        assert_eq!(
            generations(&engine, &pid, &step1).await,
            expected,
            "round {round}: a rewind iff the back won"
        );
        assert_eq!(
            generations(&engine, &pid, "step2").await,
            expected,
            "round {round}: the forward chain recreated iff the back won"
        );
        assert_eq!(
            pure_operations(&engine, &pid).await,
            0,
            "round {round}: a rewind is folded into its deciding commit, never a second hand-off"
        );

        finish_by_hand(&engine, &pid).await;
        assert_settled(&engine, &pid).await;
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}
