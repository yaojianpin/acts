//! TIMEOUT reliability cells: a step's timeout branch — the tick's one-shot
//! dispatch — under a store fault on the branch's own durable decision, a
//! client completion racing the tick that fires it, repeated ticks, a restart,
//! and the driver's passes over an already-settled run.
//!
//! The kernel keeps the legacy tick's three facts in the run's aggregate: the
//! cost start (`__started`, stamped by the owner's own start hop), the frozen
//! cost (written into the branch's scope as `__cost`, what `$cost()` reads) and
//! the one-shot marker (`__timeouts`, the branch ids that already fired). The
//! timer is a durable `DispatchTimeout` operation armed by that start hop and
//! re-woken by the recovery scan on every tick (800 ms under `cfg(test)`), so a
//! branch fires on the tick whose round admits it: one branch per round, in
//! declaration order, skipping the claimed ones and stopping at the first
//! branch still in flight.
//!
//! "Fired exactly once" is therefore a *generation* fact, read off the
//! aggregate and never off a row: the dispatch admits one execution of the
//! branch's model node and advances the marker in the same atomic commit, so a
//! re-dispatch would allocate a second generation (the read model keeps one row
//! per model node and cannot see any of it) — and the branch act's single
//! `Created` message is the external side effect the legacy cells counted. The
//! marker itself is a one-entry-per-branch list (`claim` never appends a branch
//! twice), so it pins that the claim was not torn, not that a duplicate was
//! attempted.
//!
//! Legacy's `Task::claim_timeout` wrote an in-memory claim that its tick
//! consulted; the kernel has no such per-task slot, and the *commit that
//! advances the marker is the branch's durable decision*. One difference
//! follows and the store-fault cell says so: a fault under that commit loses
//! the branch's admission too (the commit is atomic), so the branch fires when
//! the timer's next round lands, never twice — the timeout is neither lost nor
//! duplicated, which is the property the legacy cell pinned.

use std::sync::atomic::Ordering;

use serial_test::serial;

use crate::Step;
use crate::scheduler::core::commit::process::ProcessSnapshot;
use crate::scheduler::core::model::clock::{Clock, SystemClock};
use crate::scheduler::core::model::execution_graph::{ExecutionId, ExecutionState};
use crate::utils::test::{USES_IRQ, USES_MSG};

use super::*;

// ---------------------------------------------------------------------------
// Fixtures and the aggregate surfaces the timeout cells read
// ---------------------------------------------------------------------------

/// A one-step workflow whose step runs the irq act `act1` and declares one
/// timeout branch `t1`: the message act `msg1`, dispatched once its window (the
/// `window` guard, evaluated with the frozen cost `$cost()`) holds. The irq act
/// is what keeps the step live — the tick only fires the branches of a
/// `Running`/`Waiting` step.
fn one_timeout_workflow(window: &str) -> Workflow {
    let branch = Step::new()
        .with_id("t1")
        .with_if(window)
        .with_uses(USES_MSG, Vars::new().with("key", "msg1"));
    let mut step = Step::new()
        .with_id("step1")
        .with_uses(USES_IRQ, Vars::new().with("key", "act1"));
    step.timeouts.push(branch);
    let mut workflow = Workflow::new().with_id("wf-timeout-one");
    workflow.steps = vec![step];
    workflow
}

/// [`one_timeout_workflow`] with a second declared branch `t2` (`msg2`), so a
/// cell can watch the tick advance from the first to the second across rounds.
fn two_timeout_workflow(window: &str) -> Workflow {
    let mut step = Step::new()
        .with_id("step1")
        .with_uses(USES_IRQ, Vars::new().with("key", "act1"));
    for (id, key) in [("t1", "msg1"), ("t2", "msg2")] {
        step.timeouts.push(
            Step::new()
                .with_id(id)
                .with_if(window)
                .with_uses(USES_MSG, Vars::new().with("key", key)),
        );
    }
    let mut workflow = Workflow::new().with_id("wf-timeout-two");
    workflow.steps = vec![step];
    workflow
}

/// Start `workflow`, register a counter for the branch's `msg1` message, and
/// answer `(pid, "step1", counter)` once the step's act waits with every write
/// of the wait settled — the state a tick and a client completion are both
/// aimed at. The message counter counts the branch's `Created` announcements,
/// the branch's external side effect.
async fn waiting_timed_run(
    engine: &Engine,
    workflow: &Workflow,
) -> (String, String, Arc<AtomicUsize>) {
    let created = capture_created(engine, "act1");
    let fired = created_count(engine, "msg1");
    let pid = start_run(engine, workflow).await;
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
    (pid, step1, fired)
}

/// A counter of the `Created` messages keyed `key`: how many times the act was
/// announced, the side effect a duplicate branch dispatch would repeat.
fn created_count(engine: &Engine, key: &str) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let key = key.to_string();
    engine.channel().on_message(move |e| {
        let seen = seen.clone();
        let key = key.clone();
        async move {
            if e.is_params_key(&key) && e.is_state(MessageState::Created) {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    count
}

/// The run's aggregate document. A settled run keeps it even after its rows are
/// swept, so a cell reads the timeout facts here and never off a row.
async fn process_snapshot(engine: &Engine, pid: &str) -> ProcessSnapshot {
    engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the aggregate of {pid} must exist"))
}

/// The ascending generations the aggregate holds for one model node: how many
/// times the node was instantiated. A second dispatch of a fired branch — whose
/// first generation is terminal — allocates a second one (the admission policy
/// allocates for a completed slot), so this is the exactly-once surface.
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

/// The aggregate's state for one generation of one node.
async fn generation_state(
    engine: &Engine,
    pid: &str,
    tid: &str,
    generation: u64,
) -> Option<ExecutionState> {
    process_snapshot(engine, pid)
        .await
        .executions
        .iter()
        .find(|(id, _)| id.task_id == tid && id.generation == generation)
        .map(|(_, execution)| execution.state)
}

/// The branches the one-shot marker (`__timeouts`) holds for the owner node
/// `owner` of generation 0: the claim the tick wrote. It advances one branch per
/// round, and holds each branch at most once.
async fn claimed(engine: &Engine, pid: &str, owner: &str) -> Vec<String> {
    let snapshot = process_snapshot(engine, pid).await;
    let owner = ExecutionId::new(owner, 0).expect("a node id parses");
    snapshot
        .variables
        .get(&(owner, crate::utils::consts::TASK_TIMEOUTS.to_string()))
        .map(|raw| serde_json::from_str(raw).expect("the marker is a list of branch ids"))
        .unwrap_or_default()
}

/// Whether every outbox record of the run has settled (delivered or
/// quarantined): the outbox dispatch pass has no aggregate write left to make,
/// so the next write to the run's aggregate is a hop's own commit.
async fn outbox_settled(engine: &Engine, pid: &str) -> bool {
    process_snapshot(engine, pid)
        .await
        .outbox
        .values()
        .all(|record| record.state.is_terminal())
}

/// The number of executions the run's aggregate holds — the whole graph, not
/// one node: a settled run's total, so a later tick that admits anything moves
/// it.
async fn executions(engine: &Engine, pid: &str) -> usize {
    process_snapshot(engine, pid).await.executions.len()
}

/// The number of outbox records the run's aggregate holds (the kernel keeps the
/// outbox inside the aggregate): a later pass that records anything moves it.
async fn outbox_records(engine: &Engine, pid: &str) -> usize {
    process_snapshot(engine, pid).await.outbox.len()
}

/// How many settle windows ([`SETTLE`]) span `ticks` engine ticks. The driver
/// ticks once per [`crate::scheduler::runtime::TEST_TICK_MS`] under test, so this is the
/// window a "no later tick does anything" cell observes through [`poll_settle`].
fn tick_windows(ticks: u64) -> usize {
    ((crate::scheduler::runtime::TEST_TICK_MS * ticks) / SETTLE.as_millis() as u64) as usize
}

// ---------------------------------------------------------------------------
// TIMEOUT × DB失败 — a fault on the branch's own durable decision
// ---------------------------------------------------------------------------

/// A store fault under the branch's own durable decision must not lose the
/// timeout, duplicate the branch, or tear its one-shot claim. The kernel's
/// decision is the tick's aggregate commit — the branch admission and the
/// advanced marker in one atomic plan — so a refused commit applies nothing and
/// leaves the timer's operation to be re-woken: the branch fires once on the
/// next round, with one execution, one marker entry and one message.
///
/// The fault is armed on the run's aggregate (`process_key`), after the outbox
/// pass has settled so the next aggregate write is that decision and not a
/// delivery. The kernel has no per-task claim write (legacy's
/// `Task::claim_timeout` slot); the nearest durable unit is the run's aggregate
/// commit, and breaking it is what the legacy cell meant — the claim and the
/// branch together.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_store_that_refuses_the_branchs_commit_fires_it_exactly_once() {
    bounded(
        "a_store_that_refuses_the_branchs_commit_fires_it_exactly_once",
        a_store_that_refuses_the_branchs_commit_fires_it_exactly_once_inner(),
    )
    .await;
}

async fn a_store_that_refuses_the_branchs_commit_fires_it_exactly_once_inner() {
    let kv = FaultKv::new();
    let engine = engine_on(kv.store()).await;
    // A window that opens a few ticks in, so the arm lands well before the
    // round that dispatches the branch.
    let (pid, step1, fired) =
        waiting_timed_run(&engine, &one_timeout_workflow("$cost() >= 2000")).await;

    // no aggregate write is pending from the deliveries, so the next one is the
    // tick's own dispatch: the branch's durable decision
    poll_until(
        "the run's outbox to settle before arming the fault",
        || async { outbox_settled(&engine, &pid).await },
    )
    .await;

    let process = process_key(&pid);
    kv.arm_key(&process);

    // the arm fires on that commit; the one-shot fault then leaves the next
    // round to land the branch exactly once
    let what = format!("the timeout branch t1 of {pid} to fire through the fault");
    poll_until_dumped(
        &what,
        || async { generations(&engine, &pid, "t1").await == vec![0] },
        || dump_state(&engine, &pid),
    )
    .await;
    assert_eq!(kv.fired(), 1, "the armed fault must fire exactly once");
    assert_eq!(
        kv.failed_key(),
        Some(process),
        "the fault must land on the run's aggregate commit"
    );

    // exactly once: one execution, one message, one marker entry — and no
    // later tick may add to any of them
    let what = format!("the branch message of {pid} to be delivered once");
    poll_until_dumped(
        &what,
        || async { fired.load(Ordering::SeqCst) == 1 },
        || dump_state(&engine, &pid),
    )
    .await;
    assert_eq!(
        generations(&engine, &pid, "t1").await,
        vec![0],
        "the branch must have one execution"
    );
    assert_eq!(
        claimed(&engine, &pid, "step1").await,
        vec!["t1".to_string()],
        "the claim marker must hold the branch exactly once"
    );
    assert!(
        !poll_settle(
            || async {
                generations(&engine, &pid, "t1").await != vec![0]
                    || fired.load(Ordering::SeqCst) != 1
                    || claimed(&engine, &pid, "step1").await != vec!["t1".to_string()]
            },
            tick_windows(3),
        )
        .await,
        "a recovered commit must not leave the branch duplicated"
    );

    // the run converges: the fired branch settled, so completing the act ends
    // the step and the run
    complete_act(&engine, &pid, &step1).await.unwrap();
    wait_completed(&engine, &pid).await;
    assert_eq!(
        generation_state(&engine, &pid, "step1", 0).await,
        Some(ExecutionState::Completed),
        "the step must complete once its act and its branches are done"
    );
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// TIMEOUT × 并发 — a client completion racing the tick that fires the branch
// ---------------------------------------------------------------------------

/// A client completion of the timing-out act and the tick that fires its
/// timeout branch are two commits on one aggregate: whichever the engine orders
/// first, the run converges on one settled state and the branch fires at most
/// once — exactly once when the tick reached its window before the completion
/// retired the timer, never at all once the act was decided first.
///
/// The cell pins the whole space. With the window open the tick fires the
/// branch and the completion then lands under it: the step waits for the branch
/// and joins. With the window still closed the completion lands first: the step
/// completes, the timer retires and no branch fires. And in the middle the two
/// are submitted concurrently — the completion against the recovery pass that
/// would dispatch the branch — where the verdict is read back off the aggregate:
/// a branch iff the tick won, one settled state either way.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_completion_racing_the_timeout_tick_converges_exactly_once() {
    bounded(
        "a_completion_racing_the_timeout_tick_converges_exactly_once",
        a_completion_racing_the_timeout_tick_converges_exactly_once_inner(),
    )
    .await;
}

async fn a_completion_racing_the_timeout_tick_converges_exactly_once_inner() {
    // ordering 1: the tick fires the branch first, then the client completes
    {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, step1, fired) = waiting_timed_run(&engine, &one_timeout_workflow("true")).await;

        let what = format!("the timeout branch of {pid} to fire before the completion");
        poll_until_dumped(
            &what,
            || async { generations(&engine, &pid, "t1").await == vec![0] },
            || dump_state(&engine, &pid),
        )
        .await;

        complete_act(&engine, &pid, &step1).await.unwrap();
        wait_completed(&engine, &pid).await;
        let what = format!("the branch message of {pid}");
        poll_until(&what, || async { fired.load(Ordering::SeqCst) == 1 }).await;

        assert_eq!(
            generations(&engine, &pid, "t1").await,
            vec![0],
            "the fired branch must keep exactly one execution"
        );
        assert_eq!(
            generation_state(&engine, &pid, "step1", 0).await,
            Some(ExecutionState::Completed),
            "the step must join the branch and the completed act exactly once"
        );
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }

    // ordering 2: the window is still closed, so the completion retires the
    // timer before any branch can fire
    {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, step1, fired) =
            waiting_timed_run(&engine, &one_timeout_workflow("$cost() >= 100000")).await;

        complete_act(&engine, &pid, &step1).await.unwrap();
        wait_completed(&engine, &pid).await;
        assert!(
            generations(&engine, &pid, "t1").await.is_empty(),
            "a branch whose window never opened must not fire"
        );
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        assert_eq!(
            generation_state(&engine, &pid, "step1", 0).await,
            Some(ExecutionState::Completed)
        );
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }

    // ordering 3: the completion is submitted concurrently with the recovery
    // pass that would dispatch the branch — the real race, whose verdict is read
    // off the aggregate
    for round in 0..4 {
        let engine = engine_on(FaultKv::new().store()).await;
        let (pid, step1, fired) = waiting_timed_run(&engine, &one_timeout_workflow("true")).await;

        let now = SystemClock.now_ms();
        let (completed, recovered) = tokio::join!(complete_act(&engine, &pid, &step1), async {
            engine.runtime().kernel().recover(now).await
        },);
        completed.unwrap();
        recovered.unwrap();
        println!(
            "round {round}: fired={:?}",
            generations(&engine, &pid, "t1").await
        );

        wait_completed(&engine, &pid).await;
        let generations = generations(&engine, &pid, "t1").await;
        // the message rides the outbox pass, a tick behind the decision
        let what = format!("round {round}: the branch message of {pid}");
        poll_until(&what, || async {
            fired.load(Ordering::SeqCst) == generations.len()
        })
        .await;
        let claimed = claimed(&engine, &pid, "step1").await;
        let messages = fired.load(Ordering::SeqCst);
        assert!(
            generations == vec![0] || generations.is_empty(),
            "round {round}: the branch fires at most once, got {generations:?}"
        );
        assert_eq!(
            messages,
            generations.len(),
            "round {round}: one message per fired branch"
        );
        assert_eq!(
            claimed.len(),
            generations.len(),
            "round {round}: one marker entry per fired branch"
        );
        assert_eq!(
            generation_state(&engine, &pid, "step1", 0).await,
            Some(ExecutionState::Completed),
            "round {round}: the step must hold one settled decision"
        );
        sweep_until_gone(&engine, &pid).await;
        engine.close().await;
    }
}

// ---------------------------------------------------------------------------
// TIMEOUT × restart — each branch fires once across ticks and a restart
// ---------------------------------------------------------------------------

/// Across ticks and across a restart, each declared branch fires exactly once.
/// The marker is durable state of the timed-out execution, so a run recovered
/// from the store never re-fires a branch that already fired — even though its
/// step is still in flight and its branch executions are terminal instances a
/// fresh round could otherwise admit again.
///
/// The rounds are what pins it: the tick advances one branch per round in
/// declaration order (the first branch settles, the second fires), and once
/// every declared branch is claimed the timer retires. Repeated ticks after
/// that, and the fresh engine's recovery scan over the same aggregate, add
/// nothing — one execution and one message per branch, two marker entries.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn each_timeout_branch_fires_once_across_ticks_and_a_restart() {
    bounded(
        "each_timeout_branch_fires_once_across_ticks_and_a_restart",
        each_timeout_branch_fires_once_across_ticks_and_a_restart_inner(),
    )
    .await;
}

async fn each_timeout_branch_fires_once_across_ticks_and_a_restart_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let fired1 = created_count(&engine, "msg1");
    let fired2 = created_count(&engine, "msg2");
    let created = capture_created(&engine, "act1");
    let pid = start_run(&engine, &two_timeout_workflow("true")).await;
    let (_, step1) = expect_created(&created).await;
    wait_waiting_task(&engine, &pid, &step1).await;
    flush(&engine).await;

    // the tick advances one branch per round: the second only after the first
    // settled and is claimed
    let what = format!("both timeout branches of {pid} to fire");
    poll_until_dumped(
        &what,
        || async {
            generations(&engine, &pid, "t1").await == vec![0]
                && generations(&engine, &pid, "t2").await == vec![0]
        },
        || dump_state(&engine, &pid),
    )
    .await;
    let what = format!("both branch messages of {pid} to be delivered");
    poll_until(&what, || async {
        fired1.load(Ordering::SeqCst) == 1 && fired2.load(Ordering::SeqCst) == 1
    })
    .await;

    assert_eq!(
        claimed(&engine, &pid, "step1").await,
        vec!["t1".to_string(), "t2".to_string()],
        "the marker must hold every declared branch exactly once"
    );

    // later ticks create nothing: the marker is claimed and the timer retired
    assert!(
        !poll_settle(
            || async {
                generations(&engine, &pid, "t1").await != vec![0]
                    || generations(&engine, &pid, "t2").await != vec![0]
                    || fired1.load(Ordering::SeqCst) != 1
                    || fired2.load(Ordering::SeqCst) != 1
            },
            tick_windows(3),
        )
        .await,
        "a fired branch must not fire again on a later tick"
    );

    // a run recovered from the same store does not re-fire a branch: its step
    // is still in flight and its branch executions are terminal, so only the
    // durable marker keeps the fresh rounds from admitting them again
    let reloaded = restart(&engine, store.clone()).await;
    let re1 = created_count(&reloaded, "msg1");
    let re2 = created_count(&reloaded, "msg2");
    assert!(
        !poll_settle(
            || async {
                generations(&reloaded, &pid, "t1").await != vec![0]
                    || generations(&reloaded, &pid, "t2").await != vec![0]
                    || re1.load(Ordering::SeqCst) != 0
                    || re2.load(Ordering::SeqCst) != 0
            },
            tick_windows(3),
        )
        .await,
        "a restarted run must not re-fire a branch that already fired"
    );
    assert_eq!(generations(&reloaded, &pid, "t1").await, vec![0]);
    assert_eq!(generations(&reloaded, &pid, "t2").await, vec![0]);
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// TIMEOUT × 重复投递 — a fired branch is never dispatched again
// ---------------------------------------------------------------------------

/// A branch that has already fired is never dispatched again by a later tick.
/// The one-shot marker records it in the run's aggregate and the tick's round
/// skips every claimed branch, so the recovery scan that re-wakes the timer
/// each tick finds nothing to admit — the branch keeps one execution and its
/// message is sent once, not once per tick.
///
/// The duplicate the legacy tick produced was an external side effect before it
/// was ever an extra task row; the message counter here is that side effect, and
/// the generation count is the kernel's own record of the same fact.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_fired_timeout_branch_is_never_dispatched_again() {
    bounded(
        "a_fired_timeout_branch_is_never_dispatched_again",
        a_fired_timeout_branch_is_never_dispatched_again_inner(),
    )
    .await;
}

async fn a_fired_timeout_branch_is_never_dispatched_again_inner() {
    let engine = engine_on(FaultKv::new().store()).await;
    let (pid, _step1, fired) = waiting_timed_run(&engine, &one_timeout_workflow("true")).await;

    let what = format!("the timeout branch of {pid} to fire");
    poll_until_dumped(
        &what,
        || async { generations(&engine, &pid, "t1").await == vec![0] },
        || dump_state(&engine, &pid),
    )
    .await;
    let what = format!("the branch message of {pid} to be delivered once");
    poll_until(&what, || async { fired.load(Ordering::SeqCst) == 1 }).await;

    // several ticks of the same still-running step: the claimed branch is
    // skipped, so neither its execution nor its message nor its marker entry
    // moves
    assert!(
        !poll_settle(
            || async {
                generations(&engine, &pid, "t1").await != vec![0]
                    || fired.load(Ordering::SeqCst) != 1
                    || claimed(&engine, &pid, "step1").await != vec!["t1".to_string()]
            },
            tick_windows(3),
        )
        .await,
        "a fired timeout branch must not be dispatched again on a later tick"
    );
    engine.close().await;
}

// ---------------------------------------------------------------------------
// TIMEOUT × restart — the driver's passes leave a settled run alone
// ---------------------------------------------------------------------------

/// The driver's passes — the recovery scan and the outbox dispatch — leave an
/// already-settled run alone: no execution, no outbox record, no message is
/// created, and the run stays terminal across several ticks and across a
/// restart.
///
/// Legacy's `Process::do_tick` had to skip terminal tasks by inspecting their
/// state; the kernel's timer round returns `Retire` for any owner that is not
/// `Running`/`Waiting`, and a settled run's operations are all closed, so the
/// scan has nothing to re-wake. The cell states the nearest property the kernel
/// holds: the run declares a timeout branch and still gains nothing, because
/// the driver only ever drives *durable, unfinished* work. Its proof is the
/// aggregate itself — the run's total executions, its outbox records and the
/// branch's absent generation — sampled across ticks and after a fresh boot.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn the_driver_leaves_a_settled_run_alone() {
    bounded(
        "the_driver_leaves_a_settled_run_alone",
        the_driver_leaves_a_settled_run_alone_inner(),
    )
    .await;
}

async fn the_driver_leaves_a_settled_run_alone_inner() {
    let kv = FaultKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    let fired = created_count(&engine, "msg1");
    let created = capture_created(&engine, "act1");
    // a window that never opens inside the test: the branch is declared and
    // alive, so a stray pass would have something to fire if it looked too hard
    let pid = start_run(&engine, &one_timeout_workflow("$cost() >= 100000")).await;
    let (_, step1) = expect_created(&created).await;
    wait_waiting_task(&engine, &pid, &step1).await;
    flush(&engine).await;

    complete_act(&engine, &pid, &step1).await.unwrap();
    wait_completed(&engine, &pid).await;

    // the settled run's shape, read off the aggregate (a settled run's rows lag
    // the commit and are never repaired)
    assert!(
        engine
            .runtime()
            .kernel()
            .process_completed(&pid)
            .await
            .unwrap(),
        "the run must be settled"
    );
    let exec_total = executions(&engine, &pid).await;
    let outbox_total = outbox_records(&engine, &pid).await;
    assert!(
        generations(&engine, &pid, "t1").await.is_empty(),
        "the branch never fired"
    );

    // several ticks: the recovery scan and the outbox pass create nothing, the
    // run stays terminal, and no branch message appears
    assert!(
        !poll_settle(
            || async {
                !engine
                    .runtime()
                    .kernel()
                    .process_completed(&pid)
                    .await
                    .unwrap()
                    || executions(&engine, &pid).await != exec_total
                    || outbox_records(&engine, &pid).await != outbox_total
                    || fired.load(Ordering::SeqCst) != 0
                    || !generations(&engine, &pid, "t1").await.is_empty()
            },
            tick_windows(3),
        )
        .await,
        "the driver's passes must leave a settled run alone"
    );

    // the same fresh boot: recovery over the settled aggregate drives nothing
    let reloaded = restart(&engine, store.clone()).await;
    let re_fired = created_count(&reloaded, "msg1");
    assert!(
        !poll_settle(
            || async {
                !reloaded
                    .runtime()
                    .kernel()
                    .process_completed(&pid)
                    .await
                    .unwrap()
                    || executions(&reloaded, &pid).await != exec_total
                    || outbox_records(&reloaded, &pid).await != outbox_total
                    || re_fired.load(Ordering::SeqCst) != 0
                    || !generations(&reloaded, &pid, "t1").await.is_empty()
            },
            tick_windows(3),
        )
        .await,
        "a restart must leave a settled run alone"
    );
    reloaded.close().await;
}
