//! ABORT reliability cells: the `Abort` client action under a store fault, a
//! restart, a duplicate delivery, and an abort racing a completion.
//!
//! The kernel decides a client action by committing it against the aggregate
//! — one guarded batch per operation — and projects the rows from the committed
//! snapshot afterwards. `Abort` itself is a plain decision
//! (`(Running | Waiting) -> Aborted`, `runtime/core/commit/command.rs`) whose
//! plan walks the act's ancestor chain `Aborted` and skips the untouched
//! downstream. So the cells here pin:
//!
//! - a landed abort is the durable truth: recovery re-reads it, never applies
//!   it a second time, and never resurrects the decided act;
//! - a store fault the abort's commit runs into is *surfaced to the client* —
//!   the refusal consumes nothing, so the retry decides exactly once;
//! - a duplicate delivery is refused by the decision guard instead of being
//!   applied again;
//! - an abort racing a completion converges on exactly one terminal decision,
//!   with no task left non-terminal.
//!
//! Two kernel contracts shape the cells the legacy engine built around
//! `Context::cancellation_token`, and they are stated where they land:
//!
//! - the kernel runs a `run_as = Func` package *inline inside the driver operation*
//!   (`effect_plan` awaits `runner.run(request)` in `runtime/shell/mod.rs`)
//!   and attaches no cancellation to it: neither an `Abort` nor a shutdown
//!   ever reaches an in-flight effect, and a package that blocks forever
//!   wedges the engine. There is no "abort reaches the running act" surface
//!   left to pin, so the cells that asked for one pin what a stop request
//!   does hold: the act it names is decided `Aborted` (a waiting act is one
//!   node with nothing running), and no later failure of that execution's own
//!   effect can undo the decision;
//! - an abort's outbox record and its decision are written by the same guarded
//!   commit, so the legacy "record landed, application lost" window does not
//!   exist: an abort is decided exactly once, or it is refused and the run
//!   stays re-decidable.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serial_test::serial;

use crate::scheduler::core::model::task_graph::TaskState;
use crate::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Context, LegacyTaskState,
};

use super::*;

/// Start one one-step irq run on `engine`, wait for its act to wait, and answer
/// `(pid, tid)` with every write of the wait settled — the state an `Abort` is
/// aimed at.
async fn waiting_act(engine: &Engine) -> (String, String) {
    let created = capture_created(engine, "act1");
    let pid = start_run(engine, &one_step_irq("act1")).await;
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
    wait_waiting_task(engine, &pid, "s1").await;
    flush(engine).await;
    (pid, tid)
}

/// Start one two-step irq run on `engine`, wait for the first step's act to
/// wait, and answer `(pid, tid)` — the run whose abort must not leave the
/// second step alive.
async fn waiting_first_step(engine: &Engine) -> (String, String) {
    let created = capture_created(engine, "act1");
    let pid = start_run(engine, &two_step_irq_workflow()).await;
    let (announced, tid) = expect_created(&created).await;
    assert_eq!(
        announced, pid,
        "the message names the run it was created for"
    );
    assert_eq!(
        tid,
        tid_of(&pid, "step1", 0),
        "the act is addressed by its instance's wire"
    );
    wait_waiting_task(engine, &pid, "step1").await;
    flush(engine).await;
    (pid, tid)
}

/// Every execution state the run's aggregate holds for the act `tid` names,
/// one entry per generation: a decision re-applied would leave a second
/// generation, and a decision the cascade invented would be an extra entry.
/// `tid` is the act's address — the wire since C5, or the node id in tests
/// that name the node; either way the states are the node's generations.
async fn act_generations(engine: &Engine, pid: &str, tid: &str) -> Vec<TaskState> {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    let node = act_node(&snapshot, pid, tid);
    let mut states: Vec<TaskState> = snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node.as_str() == node)
        .map(|(_, task)| task.state)
        .collect();
    // HashMap iteration order is not stable; the compared form is.
    states.sort_by_key(|state| format!("{state:?}"));
    states
}

/// The aggregate's execution state for the act `tid` names (the newest
/// generation), if the aggregate still holds one.
async fn act_state(engine: &Engine, pid: &str, tid: &str) -> Option<TaskState> {
    let snapshot = engine.runtime().kernel().snapshot(pid).await.unwrap()?;
    let node = act_node(&snapshot, pid, tid);
    snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node.as_str() == node)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, task)| task.state)
}

/// The model node the address names: a wire resolves through the instance it
/// was derived from; anything else is taken as the node id itself.
fn act_node(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    pid: &str,
    tid: &str,
) -> String {
    snapshot
        .tasks
        .keys()
        .find(|id| id.wire(pid) == tid)
        .map(|id| id.node.as_str().to_owned())
        .unwrap_or_else(|| tid.to_owned())
}

/// The aggregate's state for the run's root node.
async fn root_state(engine: &Engine, pid: &str) -> Option<TaskState> {
    act_state(engine, pid, utils::consts::TASK_ROOT_TID).await
}

/// The `error.*` variables the run's aggregate holds for node `tid`: a decided
/// failure writes them, so their absence proves the act's own decision is the
/// one that stands.
async fn act_error_variables(engine: &Engine, pid: &str, tid: &str) -> Vec<String> {
    let snapshot = engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .expect("the aggregate exists");
    let mut names: Vec<String> = snapshot
        .variables
        .keys()
        .filter(|(scope, name)| {
            scope.node.as_str() == tid && (name == "error.code" || name == "error.message")
        })
        .map(|(_, name)| name.clone())
        .collect();
    names.sort();
    names
}

/// Wait until the finished run's projected rows show no non-terminal task, and
/// hand that row back for the assertions that read it.
///
/// `wait_completed` observes the *aggregate* (every execution terminal), while a
/// operation's rows are written by the task that committed it: the projection is
/// explicitly eventual (`runtime/shell/projection.rs`, re-derived on the run's
/// next commit and reclaimed whole by the sweeper once the run is delivered), so
/// a row can trail the decision by a few milliseconds — a downstream row still
/// reading `ready` while the cascade already decided it `skipped` is the shape a
/// cell would otherwise misread as live work. The callers' assertions read rows,
/// so this waits for the surface they read, never for their strength: the
/// durable truth stays the aggregate, asserted separately.
///
/// The row is captured by the poll itself, so the value returned is the one that
/// read terminal (the run's rows cannot be reclaimed before its deliveries land
/// on a tick).
async fn wait_rows_terminal(engine: &Engine, pid: &str) -> Arc<ProcRead> {
    let seen: std::sync::Mutex<Option<Arc<ProcRead>>> = std::sync::Mutex::new(None);
    let what = format!("the rows of {pid} to show no non-terminal task");
    poll_until_dumped(
        &what,
        || async {
            // The projection is written behind the write-behind queue: drain
            // what is queued before judging the rows.
            flush(engine).await;
            let Some(proc) = proc_row(engine, pid).await else {
                return false;
            };
            let terminal = proc.tasks().iter().all(|t| t.state().is_completed());
            if terminal {
                *seen.lock().expect("the settled-row slot is never poisoned") = Some(proc);
            }
            terminal
        },
        || dump_state(engine, pid),
    )
    .await;
    seen.into_inner()
        .expect("the settled-row slot is never poisoned")
        .expect("the poll returned only on a row that read terminal")
}

// ---------------------------------------------------------------------------
// ABORT × restart — a landed abort survives recovery exactly once
// ---------------------------------------------------------------------------

/// An abort that landed durably is never applied twice: a fresh engine over the
/// same store re-reads the decision instead of re-running it, the recovered run
/// holds exactly one terminal generation for the act (a re-application would
/// leave a second one), and recovery announces nothing for the decided act — a
/// fresh `Created` would invite the client to decide an act it was already told
/// the verdict of.
///
/// The legacy cell covered the window its writer opened between the durable
/// pending record and the act's own write. The kernel writes the abort's record
/// and its decision in one guarded commit (the outbox lives inside the
/// aggregate), so that window does not exist and there is nothing to re-apply:
/// the property that replaces it is the one asserted here — what landed is
/// re-read exactly once, and recovery adds no second decision.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_restart_never_reapplies_a_decided_abort() {
    bounded(
        "sch_abort_restart_never_reapplies_a_decided_abort",
        a_restart_never_reapplies_a_decided_abort_inner(),
    )
    .await;
}

async fn a_restart_never_reapplies_a_decided_abort_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, tid) = waiting_act(&engine).await;

    abort_act(&engine, &pid, &tid).await.unwrap();
    wait_completed(&engine, &pid).await;
    flush(&engine).await;

    // the decision is durable and singular before the crash
    assert_eq!(
        act_generations(&engine, &pid, &tid).await,
        vec![TaskState::Aborted],
        "the abort must decide the act exactly once, ancestors included"
    );
    assert_eq!(
        root_state(&engine, &pid).await,
        Some(TaskState::Aborted),
        "the abort must terminate the run"
    );

    // the crash a run resumes from — the run is still durable here (A11
    // reclaims a finished run only once its cleanup runs, and this cell reads
    // the decision back from the aggregate after the restart)
    engine.close().await;
    let reloaded = restart(&engine, store.clone()).await;
    let resurrected = capture_created(&reloaded, "act1");
    let announced = tokio::time::timeout(Duration::from_millis(2000), resurrected.recv()).await;
    assert!(
        announced.is_err(),
        "recovery must never announce the decided act again: {:?}",
        announced.ok()
    );

    // recovery re-read the decision: one terminal generation, no second apply
    assert_eq!(
        act_generations(&reloaded, &pid, &tid).await,
        vec![TaskState::Aborted],
        "recovery must re-read the abort, never decide it a second time"
    );
    assert_eq!(
        root_state(&reloaded, &pid).await,
        Some(TaskState::Aborted),
        "the recovered run must hold its terminal outcome"
    );
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// ABORT × normal — the act a stop request names is the act that is decided
// ---------------------------------------------------------------------------

/// An `Abort` on a waiting act (an irq act is one node that is `Waiting`;
/// nothing is running for it) is decided `Aborted` on the act itself, walks its
/// ancestors `Aborted`, and leaves no non-terminal task behind: the durable
/// decision a client's stop request asks for is exactly this one.
///
/// The legacy cell asserted that an abort *reaches a running act* through
/// `Context::cancellation_token`. The kernel runs Func effects inline on the
/// driver operation and attaches no cancellation to them, so no action can reach an
/// in-flight effect — the nearest property the kernel does hold is that the
/// stop request decides the act and the run, and that decision is what later
/// operations may not undo.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_on_a_waiting_act_decides_the_act_and_the_run() {
    bounded(
        "sch_abort_on_a_waiting_act_decides_the_act_and_the_run",
        an_abort_on_a_waiting_act_decides_the_act_and_the_run_inner(),
    )
    .await;
}

async fn an_abort_on_a_waiting_act_decides_the_act_and_the_run_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, tid) = waiting_act(&engine).await;

    abort_act(&engine, &pid, &tid).await.unwrap();
    wait_completed(&engine, &pid).await;
    flush(&engine).await;

    // the decision landed on the act the client named, exactly once
    assert_eq!(
        act_generations(&engine, &pid, &tid).await,
        vec![TaskState::Aborted],
        "the abort must decide the named act, and only once"
    );
    assert_eq!(
        root_state(&engine, &pid).await,
        Some(TaskState::Aborted),
        "the abort must terminate the run through its ancestors"
    );

    // the read model answers the decision, with nothing left to run
    let proc = wait_rows_terminal(&engine, &pid).await;
    assert!(
        proc.state().is_abort(),
        "the run must read aborted, got {}",
        proc.state()
    );
    assert_eq!(
        proc.task(&tid).map(|t| t.state()),
        Some(LegacyTaskState::Aborted),
        "the act must read its decision"
    );
    let unfinished: Vec<String> = proc
        .tasks()
        .iter()
        .filter(|t| !t.state().is_completed())
        .map(|t| format!("{}:{}", t.tid(), t.state()))
        .collect();
    assert!(
        unfinished.is_empty(),
        "an aborted run must have no non-terminal task: {unfinished:?}"
    );

    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ABORT × shutdown — a closing engine decides nothing and loses nothing
// ---------------------------------------------------------------------------

/// A shutdown never reaches an in-flight effect either (the kernel's effects run
/// inline on the driver operation, with no cancellation attached), so the property a
/// closing engine must hold is the one this cell pins: shutting down while an
/// act waits decides nothing — the durable aggregate still holds that act
/// waiting — and loses nothing, because a fresh engine over the same store
/// resumes the run with the act still waiting and still decidable. The client's
/// `Abort` after the restart is served on the recovered aggregate and decides
/// the act exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_shutdown_decides_nothing_and_the_run_resumes_undecided() {
    bounded(
        "sch_abort_shutdown_decides_nothing_and_the_run_resumes_undecided",
        a_shutdown_decides_nothing_and_the_run_resumes_undecided_inner(),
    )
    .await;
}

async fn a_shutdown_decides_nothing_and_the_run_resumes_undecided_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let (pid, tid) = waiting_act(&engine).await;

    // the engine goes away with the act waiting: nothing was running for the
    // act, and nothing decides it on the way out
    engine.close().await;

    // the crash a run resumes from (the closed engine is dropped, not closed
    // twice)
    let reloaded = engine_on(store.clone()).await;
    assert_eq!(
        act_state(&reloaded, &pid, &tid).await,
        Some(TaskState::Waiting),
        "a shutdown must not decide the waiting act: the aggregate keeps it Waiting"
    );
    let proc = read_proc(&reloaded, &pid).await;
    let waiting = waiting_task(&proc, "s1").expect("the recovered run still holds the act waiting");
    assert_eq!(waiting.state(), LegacyTaskState::Interrupt);

    // the recovered run is still the client's: the recorded action decides it
    abort_act(&reloaded, &pid, &tid).await.unwrap();
    wait_completed(&reloaded, &pid).await;
    assert_eq!(
        act_generations(&reloaded, &pid, &tid).await,
        vec![TaskState::Aborted],
        "the action served after the restart must decide the act exactly once"
    );
    assert_eq!(root_state(&reloaded, &pid).await, Some(TaskState::Aborted));
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

// A `run_as = Func` package that enters its effect, waits for the cell to
// release it, and then fails: how a cell pins the interleaving of "the abort
// decided the act while the act's own effect was still in flight" without
// depending on a token the kernel never wires.

/// The shared state between the failing package and its cell.
#[derive(Default)]
struct FailingProbe {
    started: AtomicBool,
    released: AtomicBool,
    finished: AtomicBool,
}

impl FailingProbe {
    fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// A bounded wait: the effect runs inline on the driver operation, so a cell that
    /// never releases the package must let the engine go instead of wedging it
    /// (the cell's own poll fails the test long before this).
    async fn wait_released(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.released.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

static FAILING_PROBE: LazyLock<FailingProbe> = LazyLock::new(FailingProbe::default);

#[derive(Debug, Clone)]
struct FailingProbePackage;

#[async_trait::async_trait]
impl ActPackage for FailingProbePackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "test.reliability.failing_probe",
            name: "Failing probe",
            desc: "fails after its act was aborted",
            icon: "",
            doc: "",
            version: "0.1.0",
            schema: serde_json::json!({}),
            options: None,
            run_as: ActRunAs::Func,
            resources: Vec::new(),
            catalog: ActPackageCatalog::App,
        }
    }

    fn new(_: &Config) -> crate::Result<Self> {
        Ok(Self)
    }

    async fn execute(
        &self,
        _ctx: &Context,
        _params: &serde_json::Value,
    ) -> crate::Result<Option<Vars>> {
        FAILING_PROBE.started.store(true, Ordering::SeqCst);
        FAILING_PROBE.wait_released().await;
        FAILING_PROBE.finished.store(true, Ordering::SeqCst);
        Err(crate::ActError::Package(
            "injected: the package failed after its act was decided".into(),
        ))
    }
}

/// A one-step workflow whose step runs the failing probe package.
fn failing_probe_workflow() -> Workflow {
    let mut workflow = Workflow::new().with_id("wf-failing-probe");
    workflow.steps = vec![
        crate::Step::new()
            .with_id("step1")
            .with_uses("test.reliability.failing_probe", Vars::new()),
    ];
    workflow
}

// ---------------------------------------------------------------------------
// ABORT × failure — an effect that fails cannot undo a decision that already landed
// ---------------------------------------------------------------------------

/// An act's own effect, failing after the act was already decided `Aborted`,
/// cannot undo that decision: the kernel's guard admits an action only against
/// a non-terminal execution, and the failure operation's plan is formed against the
/// pre-abort revision, so its commit is refused and recovery re-derives the
/// operation as the bare remainder — the act stays `Aborted`, with no error variables
/// and no second generation.
///
/// The legacy cell relied on `Context::cancellation_token` firing to make the
/// act fail *because* it was overridden. The kernel never cancels an in-flight
/// effect (it runs inline on the driver operation; a blocked one wedges the engine),
/// so the probe package is released by the cell instead: the effect returns its
/// error while the aggregate already holds the abort, which is the same
/// question — can the act's own failure overwrite the decision? — asked on the
/// surface the kernel has.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_failing_effect_cannot_undo_an_abort_decided_while_it_ran() {
    bounded(
        "sch_abort_failing_effect_cannot_undo_an_abort_decided_while_it_ran",
        a_failing_effect_cannot_undo_an_abort_decided_while_it_ran_inner(),
    )
    .await;
}

async fn a_failing_effect_cannot_undo_an_abort_decided_while_it_ran_inner() {
    let engine = Engine::builder()
        .add_package::<FailingProbePackage>()
        .set_store(FaultKv::new().store())
        .start()
        .await
        .unwrap();
    let pid = start_run(&engine, &failing_probe_workflow()).await;

    // the act's effect is in flight: the driver operation is inside the package
    poll_until("the failing package to enter its effect", || async {
        FAILING_PROBE.started()
    })
    .await;

    // the abort is served while the effect runs: the act is the node the client
    // names, and its execution is `Running` (an auto step)
    let abort = abort_act(&engine, &pid, &tid_of(&pid, "step1", 0)).await;
    abort.expect("the abort must be admitted while the act's effect runs");
    assert_eq!(
        act_state(&engine, &pid, "step1").await,
        Some(TaskState::Aborted),
        "the abort must decide the act even though its effect is in flight"
    );

    // the effect now returns its error; the operation that carries it is refused
    FAILING_PROBE.release();
    poll_until("the released effect to report its failure", || async {
        FAILING_PROBE.finished()
    })
    .await;

    // the decision stands, across a settle window and in the durable truth
    let what = "the aborted act to keep its decision while the failed operation is refused";
    let held = poll_settle(
        || async {
            act_state(&engine, &pid, "step1").await == Some(TaskState::Aborted)
                && root_state(&engine, &pid).await == Some(TaskState::Aborted)
        },
        50,
    )
    .await;
    assert!(
        held,
        "the act's own failure must never undo the abort: {what}"
    );
    assert_eq!(
        act_generations(&engine, &pid, "step1").await,
        vec![TaskState::Aborted],
        "the refused failure must not add a generation or move the act off Aborted"
    );
    assert!(
        act_error_variables(&engine, &pid, "step1").await.is_empty(),
        "the refused failure must not write its error onto the decided act"
    );

    // the read model answers the decision, not the effect's failure
    let proc = wait_rows_terminal(&engine, &pid).await;
    assert!(
        proc.state().is_abort(),
        "the run must read aborted, got {}",
        proc.state()
    );
    let act = proc
        .task(&tid_of(&pid, "step1", 0))
        .expect("the act's task");
    assert_eq!(act.state(), LegacyTaskState::Aborted);
    assert!(
        act.err().is_none(),
        "an overridden act must not carry the effect's error: {:?}",
        act.err()
    );

    wait_completed(&engine, &pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ABORT × store fault — a fault the abort's own commit runs into
// ---------------------------------------------------------------------------

/// A store refused at the moment of the `Abort` action must reach the client
/// instead of being swallowed: the commit is the durable truth, so a client
/// told "ok" would have lost its stop request. The refusal consumes nothing
/// (the act is still waiting), and the retry decides the act exactly once: one
/// terminal generation, the run terminal, and the finished run still sweeps.
///
/// The legacy cell failed the aborted act's own row write, a write the kernel
/// has no counterpart for: the abort is one guarded batch against the run's
/// aggregate, and the rows are only its projection afterwards. The arm
/// therefore lands on the aggregate — the durable step the client's action
/// actually runs into — and the cell proves both halves: the fault fired once
/// on that document, and the one-shot failure healed without deciding the act
/// twice.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_store_fault_at_the_aborts_durable_step_surfaces_and_heals_once() {
    bounded(
        "sch_abort_store_fault_at_the_aborts_durable_step_surfaces_and_heals_once",
        a_store_fault_at_the_aborts_durable_step_surfaces_and_heals_once_inner(),
    )
    .await;
}

async fn a_store_fault_at_the_aborts_durable_step_surfaces_and_heals_once_inner() {
    let kv = FaultKv::new();
    let engine = engine_on(kv.store()).await;
    let (pid, tid) = waiting_first_step(&engine).await;

    // the one-shot fault is aimed at the run's aggregate document: the abort's
    // commit (and the operation that carries it) is the next write of it
    let key = process_key(&pid);
    kv.arm_key(&key);
    let refused = abort_act(&engine, &pid, &tid).await;
    assert!(
        refused.is_err(),
        "a store that refuses the abort's commit must reach the client, got {refused:?}"
    );
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(key),
        "the fault must land on the run's aggregate document"
    );

    // the refusal consumed nothing: the act is still waiting and decidable
    let proc = read_proc(&engine, &pid).await;
    assert!(
        waiting_task(&proc, "step1").is_some(),
        "a refused abort must leave the act waiting"
    );
    assert_eq!(
        act_state(&engine, &pid, &tid).await,
        Some(TaskState::Waiting)
    );

    // the store healed with the one-shot fault; the retry decides exactly once
    abort_act(&engine, &pid, &tid).await.unwrap();
    assert_eq!(kv.fired(), 1, "the one-shot fault must not fire twice");
    wait_completed(&engine, &pid).await;
    flush(&engine).await;
    assert_eq!(
        act_generations(&engine, &pid, &tid).await,
        vec![TaskState::Aborted],
        "the retried abort must decide the act once, not once per attempt"
    );
    assert_eq!(root_state(&engine, &pid).await, Some(TaskState::Aborted));
    let proc = wait_rows_terminal(&engine, &pid).await;
    assert!(
        tasks_of(&proc, "step1").len() == 1,
        "no duplicate task may survive the refused abort"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ABORT × duplicate delivery — the duplicate is refused, not re-applied
// ---------------------------------------------------------------------------

/// Delivering the same `Abort` twice must decide the act exactly once: the
/// second delivery is refused by the decision guard (the act is already
/// `Aborted`, and the kernel's preflight against the authoritative snapshot
/// consumes nothing when it refuses) and changes nothing — one task for the
/// act, one generation, the same tree and the same terminal run — and the
/// finished run still sweeps.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_duplicate_abort_delivery_is_refused_without_effect() {
    bounded(
        "sch_abort_duplicate_abort_delivery_is_refused_without_effect",
        a_duplicate_abort_delivery_is_refused_without_effect_inner(),
    )
    .await;
}

async fn a_duplicate_abort_delivery_is_refused_without_effect_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, tid) = waiting_first_step(&engine).await;

    abort_act(&engine, &pid, &tid).await.unwrap();
    // let the first decision settle durably before the duplicate arrives, so
    // the duplicate is a retry and not a race with the first delivery
    wait_completed(&engine, &pid).await;

    let before = wait_rows_terminal(&engine, &pid).await;
    assert_eq!(tasks_of(&before, "step1").len(), 1, "one task for the act");
    assert_eq!(
        before.task(&tid).map(|t| t.state()),
        Some(LegacyTaskState::Aborted),
        "the first delivery leaves the act aborted"
    );

    let duplicate = abort_act(&engine, &pid, &tid).await;
    assert!(
        duplicate.is_err(),
        "a duplicate abort must never be applied again, got {duplicate:?}"
    );

    // the verdict is the only thing the duplicate produced: the tree and the
    // run's outcome are exactly what the first delivery left
    let after = wait_rows_terminal(&engine, &pid).await;
    assert_eq!(
        tasks_of(&after, "step1").len(),
        1,
        "no second task for the act"
    );
    assert_eq!(
        after.task(&tid).map(|t| t.state()),
        Some(LegacyTaskState::Aborted),
        "the duplicate may not move the decided act"
    );
    assert_eq!(
        task_snapshot(&after),
        task_snapshot(&before),
        "a refused duplicate must leave the tree untouched"
    );
    assert!(after.state().is_abort(), "the run keeps its abort");
    assert_eq!(
        act_generations(&engine, &pid, &tid).await,
        vec![TaskState::Aborted],
        "the durable truth must hold exactly one aborted generation"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// ABORT × normal — aborting a waiting act leaves no live downstream work
// ---------------------------------------------------------------------------

/// Aborting the waiting act of step1 ends the run: the run reads `aborted`, the
/// aborted act keeps its decision, no task of the run is left non-terminal, and
/// the downstream is nothing but the terminal decision the abort's own cascade
/// wrote (`Skipped`) — a single generation of it, answered by the read model as
/// such, never a waiting or running act.
///
/// The abort's ancestor walk used to race the pending scheduling, leaving an
/// unreachable `Interrupt` act downstream whose message was never emitted. The
/// kernel's abort cascade is planned against the aggregate in the same commit
/// that decides the act (the untouched downstream joins it `Skipped`), so no operation
/// can grow the run afterwards. The round loop is the regression: a live
/// downstream act — or a second generation of it — fails it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_leaves_no_live_downstream_act() {
    bounded(
        "sch_abort_leaves_no_live_downstream_act",
        an_abort_leaves_no_live_downstream_act_inner(),
    )
    .await;
}

async fn an_abort_leaves_no_live_downstream_act_inner() {
    for round in 0..40 {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, tid) = waiting_first_step(&engine).await;

        abort_act(&engine, &pid, &tid).await.unwrap();
        wait_completed(&engine, &pid).await;

        let proc = wait_rows_terminal(&engine, &pid).await;
        assert!(
            proc.state().is_abort(),
            "round {round}: the abort must end the run"
        );
        assert_eq!(
            proc.task(&tid).map(|t| t.state()),
            Some(LegacyTaskState::Aborted),
            "round {round}: the aborted act keeps its decision"
        );
        let unfinished: Vec<String> = proc
            .tasks()
            .iter()
            .filter(|t| !t.state().is_completed())
            .map(|t| format!("{}:{}", t.tid(), t.state()))
            .collect();
        assert!(
            unfinished.is_empty(),
            "round {round}: an aborted run must have no non-terminal task: {unfinished:?}"
        );

        // "no live downstream act": the downstream exists only as the decision
        // the abort's own cascade wrote (`Skipped`), one generation of it, and
        // the read model answers the same — never a waiting or running act
        let downstream = act_generations(&engine, &pid, "step2").await;
        assert_eq!(
            downstream,
            vec![TaskState::Skipped],
            "round {round}: the abort's cascade must decide the downstream exactly once, skipped"
        );
        let rows: Vec<String> = tasks_of(&proc, "step2")
            .iter()
            .map(|t| format!("{}:{}", t.tid(), t.state()))
            .collect();
        assert_eq!(
            rows,
            vec![format!(
                "{}:{}",
                tid_of(&pid, "step2", 0),
                LegacyTaskState::Skipped
            )],
            "round {round}: the downstream row must read the cascade's decision"
        );

        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}

// ---------------------------------------------------------------------------
// ABORT × concurrency — abort racing complete on the same act tid converges
// ---------------------------------------------------------------------------

/// `Next` (complete) and `Abort` fired at the same act in the same instant must
/// converge regardless of interleaving: exactly one of the two terminal
/// decisions is accepted, the loser is refused by the decision guard instead of
/// corrupting (never a panic, never a wedge), the act ends in exactly the state
/// the winner decided, and the run reaches its terminal state with exactly the
/// work the winner scheduled and every row swept.
///
/// The kernel's guarded commit makes the application exclusive: the action that
/// loses the compare-and-swap re-plans against the fresh snapshot, whose guard
/// refuses it (and a refusal is never retried). The two deliveries are spawned
/// as separate tasks — the engine serves concurrent commits of one aggregate —
/// and their spawn order alternates per round, so both winner paths are
/// exercised. The round loop is the regression: a round where both racers are
/// admitted, or the act ends in a state neither racer decided, fails it.
///
/// A red round is one of the two divergences this cell exists to catch: both
/// racers admitted (the legacy check-then-apply bug the first assertion pins), or
/// neither admitted — the kernel's own collision shape, where two concurrent
/// actions mint the same `ApplyAction` operation (`max(sequence) + 1` over the
/// operations each caller loaded) and the retry that resolves their
/// compare-and-swap re-commits an intent that already exists. It was observed as
/// both clients refused with `OperationAlreadyExists(step1, generation 0,
/// sequence 1)` and the run left `Running` with the act still `Waiting`, and it
/// is fixed by minting past taken ids. The assertion is kept as the matrix's
/// contract, never weakened to a "not both accepted" shape.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_abort_racing_a_completion_converges_on_one_terminal_decision() {
    bounded(
        "sch_abort_racing_a_completion_converges_on_one_terminal_decision",
        an_abort_racing_a_completion_converges_on_one_terminal_decision_inner(),
    )
    .await;
}

async fn an_abort_racing_a_completion_converges_on_one_terminal_decision_inner() {
    for round in 0..10 {
        let engine = engine_on(FaultKv::new().store()).await;
        // registered before the race: if the completion wins, the run announces
        // the act it scheduled
        let created2 = capture_created(&engine, "act2");
        let (pid, tid) = waiting_first_step(&engine).await;

        // the race: both deliveries target the same waiting act at once, each on
        // its own task — the engine serves concurrent commits of one aggregate
        // — with the spawn order alternated per round so both winner paths are
        // exercised (`tokio::join!` alone would always let the first-polled
        // future win)
        let spawn = |action: EventAction| {
            let rt = engine.runtime().clone();
            let pid = pid.clone();
            let tid = tid.clone();
            tokio::spawn(async move {
                rt.do_action(&Action::new(&pid, &tid, action, Vars::new()))
                    .await
            })
        };
        let (next, abort_task) = if round % 2 == 0 {
            let next = spawn(EventAction::Next);
            let abort = spawn(EventAction::Abort);
            (next, abort)
        } else {
            let abort = spawn(EventAction::Abort);
            let next = spawn(EventAction::Next);
            (next, abort)
        };
        let complete = next.await.expect("the racing task must not panic");
        let abort = abort_task.await.expect("the racing task must not panic");
        assert!(
            complete.is_ok() ^ abort.is_ok(),
            "round {round}: exactly one racer may decide the act, got complete={} abort={}",
            outcome(&complete),
            outcome(&abort)
        );
        let abort_won = abort.is_ok();

        if abort_won {
            // the abort won: the act and its ancestors are aborted and nothing
            // is left running behind it
            wait_completed(&engine, &pid).await;
            flush(&engine).await;
            assert_eq!(
                act_generations(&engine, &pid, &tid).await,
                vec![TaskState::Aborted],
                "round {round}: the raced act must end Aborted exactly once"
            );
            let proc = wait_rows_terminal(&engine, &pid).await;
            assert!(
                proc.state().is_abort(),
                "round {round}: the winning abort must terminate the run"
            );
        } else {
            // the completion won: the run's own chain is the single terminal
            // decision, and the step it scheduled is the client's to answer
            let (announced, scheduled) = expect_created(&created2).await;
            assert_eq!(announced, pid, "round {round}: the message names the run");
            assert_eq!(
                scheduled,
                tid_of(&pid, "step2", 0),
                "round {round}: the completion schedules the next step"
            );
            wait_waiting_task(&engine, &pid, "step2").await;
            complete_act(&engine, &pid, &scheduled).await.unwrap();
            wait_completed(&engine, &pid).await;
            wait_rows_terminal(&engine, &pid).await;
            assert_settled(&engine, &pid).await;
            assert_eq!(
                act_generations(&engine, &pid, &tid).await,
                vec![TaskState::Completed],
                "round {round}: the raced act must end Completed exactly once"
            );
        }

        // no task is left non-terminal, and no duplicate work exists: one task
        // for the raced act, and step2 only when the completion scheduled it
        let proc = wait_rows_terminal(&engine, &pid).await;
        let unfinished: Vec<String> = proc
            .tasks()
            .iter()
            .filter(|t| !t.state().is_completed())
            .map(|t| format!("{}:{}", t.tid(), t.state()))
            .collect();
        assert!(
            unfinished.is_empty(),
            "round {round}: no task may be left non-terminal: {unfinished:?}"
        );
        assert_eq!(
            tasks_of(&proc, "step1").len(),
            1,
            "round {round}: the race must not duplicate the act"
        );
        // the downstream is the winner's decision and nothing else: skipped
        // with the abort, completed once when the completion scheduled it
        let downstream = act_generations(&engine, &pid, "step2").await;
        let expected = if abort_won {
            vec![TaskState::Skipped]
        } else {
            vec![TaskState::Completed]
        };
        assert_eq!(
            downstream, expected,
            "round {round}: the downstream must be exactly the winner's decision"
        );

        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}
