//! PARALLEL reliability cells: the matrix's fan-out/fan-in row — one container
//! dispatch to sibling branches, joined once every branch settled — under a
//! store fault on a branch's own row, a restart with one branch in flight, a
//! duplicated branch completion, and concurrent sibling completions.
//!
//! The kernel fans a container out from the *model*: a step declaring branches
//! (`model::step_branches`) admits one child execution per selected branch in
//! one atomic plan, parks itself `Running` for the children, and the container
//! join completes it once every child is terminal.
//!
//! The container packages needed a bridge to that shape — their inline acts live
//! in the package's `in`/`acts` params, and `ctx.build_acts` writes them into
//! the headless scope the effect runner throws away
//! (`scheduler/shell/effects.rs`) — and until 2026-10-15 a
//! `uses: acts.core.parallel` step resolved its package and auto-completed with
//! no child execution, no act and no message: a silent drop, pinned by a
//! conformance cell that this round's fix had to rewrite. The bridge
//! (`materialize_container_acts`) rewrites a container step's inline acts into
//! the branches/steps the kernel fans out — one branch over the declared chain
//! for `block{ mode: sequence }`, one branch per act for `mode: parallel`, and
//! one branch holding a template step for `parallel`/`sequence{ in, acts }` —
//! and the fan-out instantiates that template once per `in` item. The cells
//! below drive both shapes: the four original ones keep driving the
//! explicit-branch fan-out, and two more cover the materialized packages (a
//! block's acts in both modes, and one template instance per `in` item with its
//! own `index`/`value` bindings).
//!
//! "Exactly once" is the aggregate's. The branch acts are executions (one per
//! model node, generation 0) and each client decision leaves one `action:` and
//! one `propagate:` record, while the container's own completion is the join's
//! single `completed:` record; a hop or a join applied twice would add a second
//! generation or a second record. The rows lag those commits and nothing repairs
//! a *settled* run's rows, so each cell reads the decision off the aggregate and
//! only the *sweep* off the read model.
//!
//! One kernel defect surfaced while porting the legacy row — the dropped param
//! fan-out above — and it is named where it bites rather than softened. A second
//! one, a hop whose effect commit lost a compare-and-swap to a concurrent client
//! commit being turned into `EffectUnknown` (manual reconciliation) instead of
//! being retried, was found by the concurrent-sibling cell and fixed during the
//! port: the losing hop now releases its claim, so the next recovery tick
//! re-drives it. That cell pins the convergence.

use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;
use serial_test::serial;

use crate::scheduler::core::commit::process::ProcessSnapshot;
use crate::scheduler::core::model::task_graph::TaskState;
use crate::{Act, Step};

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Every model node of [`parallel_workflow`] that becomes an execution: the
/// root, the container step, its two branches and their two acts. The join must
/// leave exactly this set, one generation each.
const PARALLEL_NODES: [&str; 6] = [
    utils::consts::TASK_ROOT_TID,
    "step1",
    "b1",
    "b2",
    "s1",
    "s2",
];

/// The parallel fan-out/fan-in the kernel can express: one container step with
/// two sibling branches, each branch running the same irq act (`act1`), joined
/// when both branches settle.
///
/// Both branches' acts carry the same params key (`act1`) — the legacy shape's
/// one act template instantiated twice — but each names its own node (`s1`/`s2`)
/// in its `Created` message, which is how a cell tells the two children apart
/// (the read model keeps one row per model node, so a row count cannot see a
/// fan-out at all).
fn parallel_workflow() -> Workflow {
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
                        .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                })
            })
    })
}

/// Every `Created` message of a branch act (`act1`) the engine announces, as
/// `(pid, tid)` pairs. A handler rather than [`capture_created`] because the two
/// branches share one params key: only the node id each message names separates
/// them, and a cell must see both (and every re-delivery).
fn capture_branch_acts(engine: &Engine) -> Arc<Mutex<Vec<(String, String)>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rec = seen.clone();
    engine.channel().on_message(move |e| {
        let rec = rec.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rec.lock().push((e.pid.clone(), e.tid.clone()));
            }
        }
    });
    seen
}

/// Start the container and wait until both branches' acts are waiting, answering
/// their node ids in ascending order (`("s1", "s2")`).
///
/// The wait is on the *messages* (that both acts were dispatched) and then on
/// each act's own row (that it is `Interrupt`, so a client action lands on a
/// decided node rather than racing the dispatch).
async fn wait_branches(
    engine: &Engine,
    pid: &str,
    seen: &Arc<Mutex<Vec<(String, String)>>>,
) -> (String, String) {
    let what = format!("both branch acts of {pid} to be announced");
    poll_until_dumped(
        &what,
        || async {
            let mut tids = branch_tids(seen, pid);
            tids.dedup();
            tids.len() >= 2
        },
        || dump_state(engine, pid),
    )
    .await;
    let mut tids = branch_tids(seen, pid);
    tids.dedup();
    assert_eq!(
        tids,
        vec!["s1".to_string(), "s2".to_string()],
        "the fan-out dispatches the two branch acts, one per branch"
    );
    wait_waiting_task(engine, pid, &tids[0]).await;
    wait_waiting_task(engine, pid, &tids[1]).await;
    flush(engine).await;
    (tids[0].clone(), tids[1].clone())
}

/// The sorted branch-act node ids the engine has announced for `pid`.
fn branch_tids(seen: &Arc<Mutex<Vec<(String, String)>>>, pid: &str) -> Vec<String> {
    let mut tids: Vec<String> = seen
        .lock()
        .iter()
        .filter(|(announced, _)| announced == pid)
        .map(|(_, tid)| tid.clone())
        .collect();
    tids.sort();
    tids
}

/// The run's aggregate document. A settled run keeps it even after its rows are
/// swept, so a cell reads a finished run's decision here and never off a row.
async fn process_snapshot(engine: &Engine, pid: &str) -> ProcessSnapshot {
    engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the aggregate of {pid} must exist"))
}

/// The run's executions as a sorted `(node, generation)` list: the exactly-once
/// picture a fault, a duplicate or a re-drive must not change.
async fn node_generations(engine: &Engine, pid: &str) -> Vec<(String, u64)> {
    let mut nodes: Vec<(String, u64)> = process_snapshot(engine, pid)
        .await
        .tasks
        .keys()
        .map(|id| (id.node.clone(), id.generation))
        .collect();
    nodes.sort();
    nodes
}

/// The ascending generations the aggregate holds for one model node: how many
/// times the node was instantiated. A second dispatch of a branch, or a rewind,
/// adds one.
async fn generations(engine: &Engine, pid: &str, tid: &str) -> Vec<u64> {
    let mut generations: Vec<u64> = process_snapshot(engine, pid)
        .await
        .tasks
        .keys()
        .filter(|id| id.node == tid)
        .map(|id| id.generation)
        .collect();
    generations.sort_unstable();
    generations
}

/// The aggregate's state for the newest generation of one model node.
async fn decided_state(engine: &Engine, pid: &str, tid: &str) -> Option<TaskState> {
    process_snapshot(engine, pid)
        .await
        .tasks
        .iter()
        .filter(|(id, _)| id.node == tid)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, task)| task.state)
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

/// The two record families one client decision writes: the `action` record for
/// the decision itself and the `propagate` record for the hop it set off.
fn hop_records(pid: &str, tid: &str) -> (String, String) {
    (
        format!("action:{pid}:{tid}:"),
        format!("propagate:{pid}:{tid}:"),
    )
}

/// Complete one branch act and wait until its decision *and* the hop it set off
/// are durable before returning — the settled state a duplicate delivery or a
/// crash cell aims at.
///
/// The kernel decides a client completion and propagates it through two
/// separate operations, so `complete_act` returning means only the decision is
/// durable; a cell that compares against, or crashes on, the post-completion
/// state first wants the hop to have landed.
async fn complete_act_settled(engine: &Engine, pid: &str, act: &str) {
    complete_act(engine, pid, act).await.unwrap();
    let (_, propagate) = hop_records(pid, act);
    let what = format!("the completion of {act} of {pid} to settle with its hop");
    poll_until_dumped(
        &what,
        || async {
            decided_state(engine, pid, act).await == Some(TaskState::Completed)
                && records_with_prefix(engine, pid, &propagate).await >= 1
        },
        || dump_state(engine, pid),
    )
    .await;
}

/// The join ran exactly once: every execution of the run is terminal, the
/// branch acts hold their completions, the container step is `Completed` with
/// exactly one execution, and the run holds exactly the model's nodes at
/// generation 0 — nothing was stranded, duplicated or re-dispatched.
async fn assert_join_once(engine: &Engine, pid: &str, first: &str, second: &str) {
    let unfinished: Vec<String> = process_snapshot(engine, pid)
        .await
        .tasks
        .iter()
        .filter(|(_, task)| !task.state.is_terminal())
        .map(|(id, task)| format!("{}/{}:{:?}", id.node, id.generation, task.state))
        .collect();
    assert!(
        unfinished.is_empty(),
        "the joined run must have no non-terminal execution: {unfinished:?}"
    );
    let mut expected: Vec<(String, u64)> = PARALLEL_NODES
        .iter()
        .map(|node| ((*node).to_string(), 0))
        .collect();
    expected.sort();
    assert_eq!(
        node_generations(engine, pid).await,
        expected,
        "the join must not add or drop an execution"
    );
    for node in ["step1", first, second] {
        assert_eq!(
            decided_state(engine, pid, node).await,
            Some(TaskState::Completed),
            "node {node} must hold its completion"
        );
    }
}

/// Nothing announced a fresh `Created` for the settled branch `tid` within a
/// bounded window.
///
/// A negative observation cannot be polled to a verdict, so the window is the
/// bound; the reloaded engine may re-deliver the *live* branch's message (the
/// designed at-least-once retry), but the settled branch must never be asked
/// again. Reading it off the messages is the only way: the re-announcement would
/// carry the node's own id, and no row could tell it from the live branch's.
async fn no_created_for(seen: &Arc<Mutex<Vec<(String, String)>>>, tid: &str) {
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let announced: Vec<String> = seen
        .lock()
        .iter()
        .filter(|(_, announced)| announced == tid)
        .map(|(pid, announced)| format!("{pid}/{announced}"))
        .collect();
    assert!(
        announced.is_empty(),
        "a settled branch must never be re-announced: {announced:?}"
    );
}

// ---------------------------------------------------------------------------
// PARALLEL × DB失败 — a fault under a branch's own projected row
// ---------------------------------------------------------------------------

/// A store fault under a branch row's projection must not refuse, undo or
/// duplicate the branch's work: the row is derived, so the hop that writes it
/// logs and drops the failure, the client's completion stands, both branches
/// complete, the container joins exactly once, and the finished run sweeps
/// cleanly.
///
/// Where the legacy cell broke a branch act's own durable write, the kernel's
/// nearest unit is that act's *projected row* — the branch's `tasks-id-…`
/// write. Breaking it cannot cost the decision: the aggregate is the truth and
/// the row is re-derived by a later commit of the run, or reclaimed with the
/// settled run (nothing repairs a settled run's rows), so the read model never
/// keeps a branch reading as live. A failure would mean a projection fault
/// reached the decision, or that the fault left a branch undecided or a second
/// execution behind.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_branch_row_projection_is_absorbed_and_the_join_runs_once() {
    bounded(
        "a_failed_branch_row_projection_is_absorbed_and_the_join_runs_once",
        a_failed_branch_row_projection_is_absorbed_and_the_join_runs_once_inner(),
    )
    .await;
}

async fn a_failed_branch_row_projection_is_absorbed_and_the_join_runs_once_inner() {
    let kv = FaultKv::new();
    let engine = engine_on(kv.store()).await;
    let seen = capture_branch_acts(&engine);
    let pid = start_run(&engine, &parallel_workflow()).await;
    let (first, second) = wait_branches(&engine, &pid, &seen).await;

    // Arm on the run's projected task-row family only after every setup write
    // settled, so the next row write of the run is the one the completions
    // project and the cell proves which family the fault landed on.
    let rows = format!("tasks-id-{pid}");
    kv.arm_prefix(rows.clone());
    complete_act(&engine, &pid, &first).await.unwrap();
    complete_act(&engine, &pid, &second).await.unwrap();
    flush(&engine).await;
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    let failed = kv
        .failed_key()
        .expect("a fired fault must name the write it broke");
    assert!(
        failed.starts_with(&rows),
        "the fault must land on the run's projected task rows, got {failed}"
    );

    // The decision was never the row's: the join still runs once on the two
    // completions the client was told had landed.
    wait_completed(&engine, &pid).await;
    assert_join_once(&engine, &pid, &first, &second).await;

    // The stale row is what the sweep reclaims (a projection is re-derived by a
    // later commit, and recovery never scans rows): the read model either
    // converged or disappeared with the run, never a live branch after its
    // decision.
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// PARALLEL × restart — one branch settled, the other still in flight
// ---------------------------------------------------------------------------

/// A restart over the same store with one branch completed and the other still
/// waiting must re-drive the run without re-running the finished branch: the
/// settled act is never announced again, it keeps exactly one execution and its
/// completion, the live branch is still waiting in the recovered engine, and
/// completing it there makes the container join exactly once.
///
/// The branch's completion is durable in the run's aggregate, so boot recovery
/// (which scans operations, never rows) has nothing of it to wake or re-apply;
/// only the live branch's dispatch owns an unfinished operation. A failure would
/// mean recovery re-dispatched a decided act (a second `Created` for it, or a
/// second generation), or that the join could not finish on the recovered
/// engine.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_with_a_branch_in_flight_re_drives_without_re_running_the_settled_one() {
    bounded(
        "a_restart_with_a_branch_in_flight_re_drives_without_re_running_the_settled_one",
        a_restart_with_a_branch_in_flight_re_drives_without_re_running_the_settled_one_inner(),
    )
    .await;
}

async fn a_restart_with_a_branch_in_flight_re_drives_without_re_running_the_settled_one_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let seen = capture_branch_acts(&engine);
    let pid = start_run(&engine, &parallel_workflow()).await;
    let (first, second) = wait_branches(&engine, &pid, &seen).await;

    // one branch completes durably (decision and hop included); the other stays
    // in flight, so the container is parked and the run is not settled
    complete_act_settled(&engine, &pid, &first).await;
    flush(&engine).await;

    // the crash: everything above lives in the shared store only
    let reloaded = restart(&engine, store.clone()).await;
    let seen2 = capture_branch_acts(&reloaded);

    // the live branch is still the act the recovered engine holds waiting
    wait_waiting_task(&reloaded, &pid, &second).await;
    // ... and the settled branch was never asked again (the recovered engine
    // re-delivers only the live branch's own still-open dispatch, if anything)
    no_created_for(&seen2, &first).await;
    assert_eq!(
        generations(&reloaded, &pid, &first).await,
        vec![0],
        "a settled branch keeps exactly one execution across the restart"
    );
    assert_eq!(
        decided_state(&reloaded, &pid, &first).await,
        Some(TaskState::Completed),
        "a settled branch keeps its completion across the restart"
    );
    assert!(
        !decided_state(&reloaded, &pid, &second)
            .await
            .expect("the live branch keeps its execution")
            .is_terminal(),
        "the branch the crash left in flight must still be live"
    );

    // the live branch completes in the recovered engine; the join runs once
    complete_act_settled(&reloaded, &pid, &second).await;
    wait_completed(&reloaded, &pid).await;
    assert_join_once(&reloaded, &pid, &first, &second).await;
    sweep_until_gone(&reloaded, &pid).await;
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// PARALLEL × 重复投递 — the duplicate branch completion is refused
// ---------------------------------------------------------------------------

/// Delivering the same branch completion twice must decide the branch exactly
/// once: the second delivery is refused by the decision guard (the act is
/// already `Completed`) and changes nothing — no second generation, no second
/// `action` or `propagate` record, no second execution — and the other branch
/// still completes, so the container joins exactly once.
///
/// A failure would mean a duplicate could re-decide a settled act: a second
/// generation for its node, a second hop record, or a join that ran twice.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_branch_completion_is_refused_without_a_second_generation() {
    bounded(
        "a_duplicate_branch_completion_is_refused_without_a_second_generation",
        a_duplicate_branch_completion_is_refused_without_a_second_generation_inner(),
    )
    .await;
}

async fn a_duplicate_branch_completion_is_refused_without_a_second_generation_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let seen = capture_branch_acts(&engine);
    let pid = start_run(&engine, &parallel_workflow()).await;
    let (first, second) = wait_branches(&engine, &pid, &seen).await;

    // the first delivery decides the branch; let both its hops settle durably
    // before the duplicate arrives, so the duplicate is a retry and not a race
    // with the decision or with the propagation it set off
    complete_act_settled(&engine, &pid, &first).await;
    let (action, propagate) = hop_records(&pid, &first);
    flush(&engine).await;

    let before = (
        node_generations(&engine, &pid).await,
        records_with_prefix(&engine, &pid, &action).await,
        records_with_prefix(&engine, &pid, &propagate).await,
    );
    assert_eq!(before.1, 1, "the completion leaves one action record");
    assert_eq!(
        before.2, 1,
        "the completion's hop leaves one propagate record"
    );

    let duplicate = complete_act(&engine, &pid, &first).await;
    assert!(
        duplicate.is_err(),
        "a duplicate branch completion must be refused, got {duplicate:?}"
    );

    // the refusal consumed nothing: no second generation, no second record
    assert_eq!(
        node_generations(&engine, &pid).await,
        before.0,
        "a refused duplicate must not add an execution"
    );
    assert_eq!(
        records_with_prefix(&engine, &pid, &action).await,
        before.1,
        "a refused duplicate must not add an action record"
    );
    assert_eq!(
        records_with_prefix(&engine, &pid, &propagate).await,
        before.2,
        "a refused duplicate must not add a propagate record"
    );
    assert_eq!(
        generations(&engine, &pid, &first).await,
        vec![0],
        "the branch keeps exactly one generation"
    );

    // the other branch completes; the join runs once
    complete_act_settled(&engine, &pid, &second).await;
    wait_completed(&engine, &pid).await;
    assert_join_once(&engine, &pid, &first, &second).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// PARALLEL × 并发 — the two siblings complete at once
// ---------------------------------------------------------------------------

/// The two siblings completing in the same instant must advance the container
/// exactly once: both client actions are accepted (each only needs its own
/// waiting act, and neither is the other's guard), the join sees both children
/// terminal and completes the container once, and the run settles and sweeps.
///
/// The two racers are the client completion and the driver's own hop for the
/// *other* branch: whichever hop commits second loses the aggregate's
/// compare-and-swap. That is not a decision — a conflict applies nothing — so
/// the losing hop releases its claim and is re-driven from the fresh snapshot on
/// the next recovery tick, and the join still fires exactly once. (While porting
/// this row the losing hop stayed `CommitPrepared` until its lease expired and
/// was then parked `EffectUnknown` — manual reconciliation — stranding the run
/// in 4 of 6 runs; the fix is what this cell pins.)
///
/// A failure would mean a concurrent pair of completions stranded the join (a
/// container that never finished), ran it twice (a second container execution),
/// or left a task non-terminal.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_sibling_completions_join_the_container_exactly_once() {
    bounded(
        "concurrent_sibling_completions_join_the_container_exactly_once",
        concurrent_sibling_completions_join_the_container_exactly_once_inner(),
    )
    .await;
}

async fn concurrent_sibling_completions_join_the_container_exactly_once_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let seen = capture_branch_acts(&engine);
    let pid = start_run(&engine, &parallel_workflow()).await;
    let (first, second) = wait_branches(&engine, &pid, &seen).await;

    let (left, right) = tokio::join!(
        complete_act(&engine, &pid, &first),
        complete_act(&engine, &pid, &second),
    );
    assert!(
        left.is_ok() && right.is_ok(),
        "both siblings must be decided: {} / {}",
        outcome(&left),
        outcome(&right)
    );

    wait_completed(&engine, &pid).await;
    assert_join_once(&engine, &pid, &first, &second).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// PARALLEL × 容器包 — the packages' inline acts as kernel sub-executions
// ---------------------------------------------------------------------------

/// The `acts.core.{parallel,sequence}{ in, acts }` shape: one container step
/// whose fan-out lives in the package's `in`/`acts` params, over two items the
/// run supplies as a workflow variable (the expression form, like
/// `examples/approve`'s `in: '${{ pm }}'`). Each item instantiates the
/// container's template step once — the legacy engine's per-item block task —
/// and the act reads its item through the scope chain
/// (`${{ value }}`/`${{ index }}`), exactly as the legacy options carried it.
/// The two packages differ only in how the items are admitted (together vs
/// chained), which is what [`item_predecessors`] reads back.
fn package_items_workflow(package: &str) -> Workflow {
    let mut step = Step::new();
    step.id = "step1".to_string();
    step.uses = Some(package.to_string());
    step.params = json!({
        "in": "${{ items }}",
        "acts": [Act::msg(|act| act.with_params_vars(|v| v
            .with("key", "act1")
            .with("value", "${{ value }}")
            .with("index", "${{ index }}")
        ))]
    });
    let mut workflow = workflow_with(step);
    workflow
        .vars
        .push(crate::Variant::create("items", json!(["u1", "u2"])));
    workflow
}

/// A one-step workflow over `step`. The fixtures below build their step
/// programmatically (their params differ), which `Workflow::with_step` cannot
/// express: it takes a `fn` pointer, and a builder closure would capture the
/// model it is building.
fn workflow_with(step: Step) -> Workflow {
    let mut workflow = Workflow::new();
    workflow.steps.push(step);
    workflow
}

/// The `acts.core.block{ mode, acts }` shape: one container step whose inline
/// acts are the package's `params.acts` array. Legacy built them into the task
/// tree from the act's own `run`; the kernel materializes the same children
/// into the model at import, so the two modes below are the two shapes that
/// rewrite takes — one branch over the declared chain (sequence), one branch
/// per act (parallel).
fn package_block_workflow(mode: &str) -> Workflow {
    let mut step = Step::new();
    step.id = "step1".to_string();
    step.uses = Some("acts.core.block".to_string());
    step.params = json!({
        "mode": mode,
        "acts": [
            Act::msg(|act| act.with_params_vars(|v| v.with("key", "block1"))),
            Act::msg(|act| act.with_params_vars(|v| v.with("key", "block2"))),
        ]
    });
    workflow_with(step)
}

/// One `Created` act message a cell observed: the model node it names and the
/// item facts its params resolved to.
#[derive(Clone, Debug)]
struct AnnouncedAct {
    nid: String,
    key: String,
    value: String,
    index: String,
}

/// The messages a cell has seen, shared with the engine's handler.
type Announced = Arc<Mutex<Vec<AnnouncedAct>>>;

/// Every `Created` act message the engine announces, with the params a
/// container package's act resolved (`key`/`value`/`index`) — what a param
/// fan-out's per-item act resolves its item to. The items share one act model
/// node (they are two generations of it), so the node id alone cannot tell them
/// apart and the resolved params must.
fn capture_act_items(engine: &Engine) -> Announced {
    let seen: Announced = Arc::new(Mutex::new(Vec::new()));
    let rec = seen.clone();
    engine.channel().on_message(move |e| {
        let rec = rec.clone();
        async move {
            if !e.is_state(MessageState::Created) {
                return;
            }
            let Some(params) = e.inputs.get::<Vars>("params") else {
                return;
            };
            // A string param renders as its text (that is what the model
            // declared); anything else keeps its JSON spelling.
            let text = |name: &str| match params.get::<serde_json::Value>(name) {
                Some(serde_json::Value::String(text)) => text,
                Some(value) => value.to_string(),
                None => String::new(),
            };
            rec.lock().push(AnnouncedAct {
                nid: e.nid.clone(),
                key: text("key"),
                value: text("value"),
                index: text("index"),
            });
        }
    });
    seen
}

/// The keys of the `Created` act messages the engine announced, sorted: how a
/// cell names the acts a container drove, without knowing their assigned ids.
fn announced_keys(seen: &Announced) -> Vec<String> {
    let mut keys: Vec<String> = seen
        .lock()
        .iter()
        .map(|act| act.key.clone())
        .filter(|key| !key.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// The model node a run instantiates per `in` item, and the generations the
/// aggregate holds for it: the shape a param fan-out must produce, one per item.
async fn item_generations(engine: &Engine, pid: &str) -> Vec<u64> {
    generations(engine, pid, "step1#item").await
}

/// The `(index, value)` bindings the param fan-out left on the template
/// executions' own scopes, sorted by index: the item facts the acts read.
async fn item_bindings(engine: &Engine, pid: &str) -> Vec<(String, String)> {
    let snapshot = process_snapshot(engine, pid).await;
    let mut bindings: Vec<(String, String)> = snapshot
        .tasks
        .keys()
        .filter(|id| id.node == "step1#item")
        .map(|id| {
            let read = |name: &str| {
                snapshot
                    .variables
                    .get(&(id.clone(), name.to_string()))
                    .cloned()
                    .unwrap_or_default()
            };
            (read("index"), read("value"))
        })
        .collect();
    bindings.sort();
    bindings
}

/// The predecessor edges of the template executions, by generation: the fact
/// that tells the two item shapes apart. `parallel` sources every item from
/// the container; `sequence` chains each behind the previous item (the legacy
/// `build_acts(&acts, true)`), which is why its items start one at a time.
async fn item_predecessors(engine: &Engine, pid: &str) -> Vec<(u64, Vec<String>)> {
    let snapshot = process_snapshot(engine, pid).await;
    let mut edges: Vec<(u64, Vec<String>)> = snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node == "step1#item")
        .map(|(id, task)| {
            (
                id.generation,
                task.predecessors
                    .iter()
                    .map(|parent| format!("{}@{}", parent.node, parent.generation))
                    .collect(),
            )
        })
        .collect();
    edges.sort();
    edges
}

/// The `in`/`acts` param fan-out is a kernel sub-execution now: the container
/// admits one execution of its template step per `in` item, the item's
/// `index`/`value` land on that execution's own scope, the template's acts
/// (materialized from the package's params at import) run under each item, and
/// the container joins once every item settled.
///
/// The occurrences share the template's model node and take successive
/// generations of it: legacy built one block task per item with a fresh id, and
/// the kernel's equivalent is one node instantiated N times (`plan_fanout`
/// hands a repeated task id the generation after the plan's own last
/// allocation, since the snapshot cannot see this plan's creations yet). A
/// failure would mean an item was dropped or duplicated, its bindings mixed up,
/// the wrong admission shape, or a container that never joined.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_uses_param_fan_out_instantiates_one_template_per_item() {
    bounded(
        "a_uses_param_fan_out_instantiates_one_template_per_item",
        a_uses_param_fan_out_instantiates_one_template_per_item_inner(),
    )
    .await;
}

async fn a_uses_param_fan_out_instantiates_one_template_per_item_inner() {
    // `parallel` admits the items together, `sequence` one behind the other:
    // the same per-item instantiation, two admission shapes.
    for (package, chained) in [("acts.core.parallel", false), ("acts.core.sequence", true)] {
        let engine = engine_on(FaultKv::new().store()).await;
        let seen = capture_act_items(&engine);
        let pid = start_run(&engine, &package_items_workflow(package)).await;

        wait_completed(&engine, &pid).await;
        assert_eq!(
            item_generations(&engine, &pid).await,
            vec![0, 1],
            "({package}) one template execution per `in` item, at successive generations"
        );
        assert_eq!(
            item_bindings(&engine, &pid).await,
            vec![
                ("0".to_string(), "\"u1\"".to_string()),
                ("1".to_string(), "\"u2\"".to_string()),
            ],
            "({package}) each item's index and value land on its own template scope"
        );
        let source = "step1@0".to_string();
        let expected: Vec<Vec<String>> = if chained {
            vec![vec![source], vec!["step1#item@0".to_string()]]
        } else {
            vec![vec![source.clone()], vec![source]]
        };
        assert_eq!(
            item_predecessors(&engine, &pid)
                .await
                .into_iter()
                .map(|(_, edges)| edges)
                .collect::<Vec<_>>(),
            expected,
            "({package}) the items are admitted as the package declares"
        );
        // The template's act ran once per item, each resolving its own item
        // through the scope chain: the same node, two values. The messages ride
        // the outbox to the emitter's per-process worker, which may still be
        // delivering when the run settles, so wait for both before asserting.
        let announced_items =
            |seen: &Announced| seen.lock().iter().filter(|act| act.key == "act1").count();
        let what = format!("({package}) the act of each item of {pid} to be announced");
        poll_until_dumped(
            &what,
            || async { announced_items(&seen) >= 2 },
            || dump_state(&engine, &pid),
        )
        .await;
        let announced = seen.lock().clone();
        let mut items: Vec<String> = announced
            .iter()
            .filter(|act| act.key == "act1")
            .map(|act| format!("{}={}", act.index, act.value))
            .collect();
        // The two items' hops are independent, so their deliveries may arrive
        // in either order: the cell asserts the set, not the arrival order.
        items.sort();
        assert_eq!(
            items,
            vec!["0=u1".to_string(), "1=u2".to_string()],
            "({package}) the act of each item resolves that item's `index`/`value`: {announced:?}"
        );
        assert_eq!(
            decided_state(&engine, &pid, "step1").await,
            Some(TaskState::Completed),
            "({package}) the container completes once its items settled"
        );

        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}

/// The block's inline acts are model children now: both declared acts run and
/// the run completes, in both modes. Legacy expanded the same array through the
/// package's `ctx.build_acts`, which the kernel's effect seam discards — so
/// before the import materialized them, a block step completed with no child at
/// all and neither message was ever announced.
///
/// A failure would mean one of the two modes dropped its acts, ran one twice, or
/// never joined.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_block_packages_inline_acts_run_as_children_in_both_modes() {
    bounded(
        "a_block_packages_inline_acts_run_as_children_in_both_modes",
        a_block_packages_inline_acts_run_as_children_in_both_modes_inner(),
    )
    .await;
}

async fn a_block_packages_inline_acts_run_as_children_in_both_modes_inner() {
    for mode in ["sequence", "parallel"] {
        let engine = engine_on(FaultKv::new().store()).await;
        let seen = capture_act_items(&engine);
        let pid = start_run(&engine, &package_block_workflow(mode)).await;

        wait_completed(&engine, &pid).await;
        // The messages ride the outbox and are handed to the emitter's
        // per-process worker, which may still be delivering when the run
        // settles: wait for what the cell reads, then assert.
        let what = format!("({mode}) both inline acts of {pid} to be announced");
        poll_until_dumped(
            &what,
            || async { announced_keys(&seen).len() >= 2 },
            || dump_state(&engine, &pid),
        )
        .await;
        assert_eq!(
            announced_keys(&seen),
            vec!["block1".to_string(), "block2".to_string()],
            "({mode}) both inline acts must run"
        );
        let unfinished: Vec<String> = process_snapshot(&engine, &pid)
            .await
            .tasks
            .iter()
            .filter(|(_, task)| !task.state.is_terminal())
            .map(|(id, task)| format!("{id:?}:{:?}", task.state))
            .collect();
        assert!(
            unfinished.is_empty(),
            "({mode}) the joined run must have no non-terminal execution: {unfinished:?}"
        );
        assert_eq!(
            decided_state(&engine, &pid, "step1").await,
            Some(TaskState::Completed),
            "({mode}) the container completes once its acts settled"
        );

        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}
