//! Reliability-condition tests for the kernel matrix: each feature row crossed
//! with store failure, restart, duplicate delivery, and concurrency.
//!
//! What is under test is the kernel engine, so a cell observes the kernel's own
//! surfaces. A run's durable truth is its aggregate document; the
//! `procs`/`tasks`/`vars` rows are that document's projection, and the driver
//! advances the run's operations on its own task. A cell therefore reads the run
//! through [`read_proc`] (the read model, rebuilt from the rows), the row
//! collections, the engine's own messages, and the run's env — never through a
//! resident process object that keeps itself current.
//!
//! The fault backends are the store seams a cell drives: fail one write
//! ([`FaultKv`]), refuse everything for a while ([`OutageKv`]), cut every write
//! from an exact boundary ([`CutKv`]), or park selected reads to force an
//! interleaving ([`GateKv`]). Arming one on the right key family is the cell's
//! job — the families, with the process segment hex-encoded (see
//! `document_store::encoded_process_key`):
//!
//! | family | key |
//! |---|---|
//! | the authoritative aggregate | [`process_key`] |
//! | the recovery / outbox indexes | `kernel/indexes/{recovery,outbox}/<hex pid>` |
//! | the projected proc row | `procs-id-<pid>` |
//! | a projected task row | `tasks-id-<pid><tid>` |
//! | a task's projected vars row | `vars-id-<pid><tid>` |
//! | a channel message / delivery row | `messages-…` / `deliveries-…` |
//!
//! (The kernel keeps its outbox records inside the aggregate, so `ops` rows
//! stay empty; a legacy `ops-id-…` arm belongs to the aggregate family now.)
//!
//! Two kernel contracts differ from the legacy engine's, and a cell that arms a
//! fault has to know them:
//!
//! - a row write is a *projection*: its failure is logged and dropped, never
//!   propagated into the operation, and the row is re-derived from the aggregate by
//!   the next commit of that run. Nothing else repairs a row — recovery scans
//!   operations, not rows — so a cell that breaks the *last* row write of a run
//!   is testing whether that run still converges on its own;
//! - an aggregate or index write is the durable truth: its failure reaches the
//!   caller (the client action returns `Err`, the operation is retried or parked by
//!   recovery), and the run must converge exactly once anyway.
//!
//! `#![allow(dead_code)]`: the harness is shared by every feature file, and a
//! helper three cells need is not used by the other nine.
#![allow(dead_code)]

pub(crate) mod abort;
pub(crate) mod action;
pub(crate) mod back;
pub(crate) mod cancel;
pub(crate) mod error;
pub(crate) mod next;
pub(crate) mod parallel;
pub(crate) mod remove;
pub(crate) mod stall;
pub(crate) mod subflow;
pub(crate) mod timeout;
pub(crate) mod timer;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use crate::{
    ActError, Action, Config, Engine, KvStore, MemoryStore, MessageState, Result, ScanOptions,
    Signal, StoreBatchOp, StoreGuard, Vars, Workflow,
    event::EventAction,
    query::{Expr, Filter, Query},
    scheduler::{Process, Task},
    utils,
    utils::test::USES_IRQ,
};

/// Run a test body under a hard deadline. A driver stall — the failure mode a
/// kernel cell is most likely to hit — must fail the test instead of freezing
/// the serial queue behind it.
pub(crate) async fn bounded<F: std::future::Future<Output = ()>>(what: &str, fut: F) {
    // Generous on purpose: a racing cell runs dozens of rounds, so the bound
    // must only catch a genuine stall (a wedged driver), not a loaded machine
    // finishing its rounds slowly — and the full suite runs these cells beside
    // ~850 others, many of them engine tests of their own, so the box is
    // oversubscribed while they wait on a driver tick.
    tokio::time::timeout(Duration::from_secs(600), fut)
        .await
        .unwrap_or_else(|_| panic!("{what} stalled: no progress within 600s (engine stall?)"));
}

/// Longest any settle poll waits; every poll asserts its deadline instead of
/// letting a hang look like a pass. The bound is the *load* a poll has to
/// survive, not the happy-path latency: the full suite runs ~850 other tests
/// (many of them engines with their own driver ticks) beside these cells.
pub(crate) const WAIT: Duration = Duration::from_secs(90);
pub(crate) const TICK: Duration = Duration::from_millis(20);

/// Longest the sweep poll waits: a delivery claim whose confirmation was lost
/// is only taken over when its lease expires (`DELIVERY_LEASE_MS`, 30 s), and
/// the sweep is what waits on that record, so its bound has to outlive the
/// lease — plus the same loaded-machine headroom as [`WAIT`].
pub(crate) const SWEEP_WAIT: Duration = Duration::from_secs(120);

/// Poll `ready` until it holds or `WAIT` runs out. `what` names the awaited
/// condition: the deadline panic carries it, so a stall that only shows up
/// under CI's load says which condition starved instead of a bare deadline.
pub(crate) async fn poll_until<F, Fut>(what: &str, ready: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    poll_until_dumped(what, ready, || async { String::new() }).await
}

/// [`poll_until`] that also prints `dump()` once the wait passes a few seconds
/// (and again whenever the dumped state changes): a condition that only stalls
/// under CI's load ships its live row evidence home in the failure output,
/// instead of leaving the next session to guess which state ate the work.
pub(crate) async fn poll_until_dumped<F, Fut, D, DumpFut>(what: &str, ready: F, dump: D) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
    D: Fn() -> DumpFut,
    DumpFut: Future<Output = String>,
{
    poll_until_within(what, ready, dump, WAIT).await
}

/// [`poll_until_dumped`] with the caller's own bound: the sweep waits on a
/// delivery claim the engine may only be able to take over when its lease
/// expires (see [`SWEEP_WAIT`]).
pub(crate) async fn poll_until_within<F, Fut, D, DumpFut>(
    what: &str,
    mut ready: F,
    dump: D,
    within: Duration,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
    D: Fn() -> DumpFut,
    DumpFut: Future<Output = String>,
{
    let start = Instant::now();
    let deadline = start + within;
    let mut last = String::new();
    loop {
        if ready().await {
            return true;
        }
        let elapsed = start.elapsed();
        if elapsed > Duration::from_secs(5) {
            let state = dump().await;
            if state != last {
                println!("still waiting for {what} after {elapsed:?}: {state}");
                last = state;
            }
        }
        assert!(Instant::now() < deadline, "{what}: poll deadline passed");
        tokio::time::sleep(TICK).await;
    }
}

/// How long a settle poll waits between rounds.
pub(crate) const SETTLE: Duration = Duration::from_millis(20);

/// Poll `ready` until it holds or `ticks` settle windows run out. Every caller
/// asserts the returned verdict, so a stuck engine fails the test instead of
/// passing vacuously.
pub(crate) async fn poll_settle<F, Fut>(mut ready: F, ticks: usize) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..ticks {
        if ready().await {
            return true;
        }
        tokio::time::sleep(SETTLE).await;
    }
    false
}

/// `Ok`/`Err` of a raced action as a short printable outcome.
pub(crate) fn outcome(r: &Result<()>) -> String {
    match r {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("err({e})"),
    }
}

// ---------------------------------------------------------------------------
// The engine, over a caller-owned store
// ---------------------------------------------------------------------------

/// An engine over `store`: the cell owns the store, so it can arm faults on it
/// and restart the engine against the same durable state.
pub(crate) async fn engine_on(store: Arc<dyn KvStore>) -> Engine {
    Engine::builder().set_store(store).start().await.unwrap()
}

/// The same engine with the cell's own config (a slot cap, a tick).
pub(crate) async fn engine_with(store: Arc<dyn KvStore>, config: &Config) -> Engine {
    Engine::builder()
        .set_config(config)
        .set_store(store)
        .start()
        .await
        .unwrap()
}

/// Close `engine` and start a fresh one over the same store: the crash a run
/// resumes from. The new driver's boot pass is what recovers the run — a
/// kernel engine never trusts anything but the store it boots on.
pub(crate) async fn restart(engine: &Engine, store: Arc<dyn KvStore>) -> Engine {
    engine.close().await;
    engine_on(store).await
}

/// Start `workflow` and answer the pid it runs under.
pub(crate) async fn start_run(engine: &Engine, workflow: &Workflow) -> String {
    start_run_with(engine, workflow, Vars::new()).await
}

/// Start `workflow` with `vars` and answer the pid it runs under.
pub(crate) async fn start_run_with(engine: &Engine, workflow: &Workflow, vars: Vars) -> String {
    let proc = engine.runtime().start(workflow, vars).await.unwrap();
    proc.id().to_string()
}

// ---------------------------------------------------------------------------
// The read model
// ---------------------------------------------------------------------------

/// The run's projected row as a `Process`, or `None` while it has no row (a
/// settled run whose rows the sweeper reclaimed).
pub(crate) async fn proc_row(engine: &Engine, pid: &str) -> Option<Arc<Process>> {
    engine.runtime().proc(pid).await.unwrap()
}

/// The run's projected row, failing the test when there is none.
pub(crate) async fn read_proc(engine: &Engine, pid: &str) -> Arc<Process> {
    proc_row(engine, pid)
        .await
        .unwrap_or_else(|| panic!("process {pid} has no row"))
}

/// The run's projected tasks for the model node `nid` (the read model keeps one
/// task per model node; a `uses` step is one node, so an irq act is the step's
/// own task).
pub(crate) fn tasks_of(proc: &Arc<Process>, nid: &str) -> Vec<Arc<Task>> {
    proc.task_by_nid(nid)
}

/// The model node an address names: a wire (the C5 instance id) resolves
/// through the instance it was derived from; anything else is taken as the
/// node id itself. Aggregate-side helpers use this so a test may pass either
/// form.
pub(crate) fn node_of(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    pid: &str,
    address: &str,
) -> String {
    snapshot
        .tasks
        .keys()
        .find(|id| id.wire(pid) == address)
        .map(|id| id.node.as_str().to_owned())
        .unwrap_or_else(|| address.to_owned())
}

/// The waiting task of the node `nid` (`Interrupt`), if it is waiting.
pub(crate) fn waiting_task(proc: &Arc<Process>, nid: &str) -> Option<Arc<Task>> {
    tasks_of(proc, nid)
        .into_iter()
        .find(|t| t.state().is_interrupted())
}

/// Poll until the node `nid` is waiting, and return its task.
pub(crate) async fn wait_waiting_task(engine: &Engine, pid: &str, nid: &str) -> Arc<Task> {
    let what = format!("node '{nid}' of {pid} to be waiting");
    poll_until_dumped(
        &what,
        || async {
            proc_row(engine, pid)
                .await
                .is_some_and(|proc| waiting_task(&proc, nid).is_some())
        },
        || dump_state(engine, pid),
    )
    .await;
    let proc = read_proc(engine, pid).await;
    waiting_task(&proc, nid).expect("the waiting task must still be there")
}

/// The run's projected task for the model node `nid`, failing the test when
/// there is none.
pub(crate) async fn read_task(engine: &Engine, pid: &str, nid: &str) -> Arc<Task> {
    let proc = read_proc(engine, pid).await;
    proc.task(nid)
        .unwrap_or_else(|| panic!("run {pid} has no task row for node {nid}"))
}

/// The run reached a business-success end and no task of it is left in a
/// non-terminal state: nothing was left half-decided by a fault or a race.
pub(crate) async fn assert_settled(engine: &Engine, pid: &str) {
    let proc = read_proc(engine, pid).await;
    assert!(
        proc.state().is_biz_success(),
        "run {pid} must settle in business success, got {}",
        proc.state()
    );
    let unfinished: Vec<String> = proc
        .tasks()
        .iter()
        .filter(|t| !t.state().is_completed())
        .map(|t| format!("{}:{}", t.id, t.state()))
        .collect();
    assert!(
        unfinished.is_empty(),
        "a completed run must have no non-terminal task: {unfinished:?}"
    );
}

/// The run's live task set as a sorted `(task id, state)` list — the
/// comparable snapshot of a decision point.
pub(crate) fn task_snapshot(proc: &Arc<Process>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = proc
        .tasks()
        .iter()
        .map(|t| (t.id.clone(), t.state().to_string()))
        .collect();
    v.sort();
    v
}

/// Wait until the run's projected task set stops moving: the same snapshot on
/// three consecutive ticks. A decision is committed by the acting task while
/// the driver's own operations keep landing, so a comparison taken immediately would
/// race them.
pub(crate) async fn settle_tasks(engine: &Engine, pid: &str) {
    let deadline = Instant::now() + WAIT;
    let mut last = task_snapshot(&read_proc(engine, pid).await);
    let mut stable = 0;
    loop {
        tokio::time::sleep(TICK).await;
        let now = task_snapshot(&read_proc(engine, pid).await);
        if now == last {
            stable += 1;
            if stable >= 3 {
                return;
            }
        } else {
            stable = 0;
            last = now.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the task set never settled: {now:?}"
        );
    }
}

/// Poll until every execution of the run is terminal.
pub(crate) async fn wait_completed(engine: &Engine, pid: &str) {
    let what = format!("run {pid} to settle");
    poll_until_dumped(
        &what,
        || async {
            engine
                .runtime()
                .kernel()
                .process_completed(pid)
                .await
                .unwrap_or(false)
        },
        || dump_state(engine, pid),
    )
    .await;
}

/// A coarse, stable picture of the run for the failure output: the row's own
/// state and its tasks, the run's operation rows and the un-settled delivery
/// rows. No timestamps, so a genuinely stuck state prints once.
pub(crate) async fn dump_state(engine: &Engine, pid: &str) -> String {
    let store = engine.runtime().store();
    let proc = proc_row(engine, pid).await;
    let mut out = match &proc {
        Some(proc) => format!("proc={} tasks[", proc.state()),
        None => "proc=<no row> tasks[".to_string(),
    };
    if let Some(proc) = &proc {
        let mut tasks: Vec<String> = proc
            .tasks()
            .iter()
            .map(|t| format!("{}:{}", t.id, t.state()))
            .collect();
        tasks.sort();
        out.push_str(&tasks.join(","));
    }
    out.push(']');
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    let ops = store
        .ops()
        .query(&q)
        .await
        .map(|r| r.rows.len())
        .unwrap_or(0);
    let deliveries = store
        .deliveries()
        .query(&q)
        .await
        .map(|r| r.rows.len())
        .unwrap_or(0);
    out.push_str(&format!(" ops={ops} deliveries={deliveries}"));
    // The raw proc row: `removable` is what the sweeper selects on, and a
    // lost projection write is visible here as a stale flag.
    match store.procs().find(pid).await {
        Ok(row) => out.push_str(&format!(
            " proc-row={}/removable={}",
            row.state, row.removable
        )),
        Err(_) => out.push_str(" proc-row=gone"),
    }
    // The run's own outbox records — the family a stalled or unconfirmed
    // delivery lives in, and the one `removable` is decided from. Coarse tags
    // only (no timestamps), so a stuck state prints once.
    if let Ok(Some(snapshot)) = engine.runtime().kernel().snapshot(pid).await {
        use crate::scheduler::core::outbox::records::OutboxState;
        let mut outbox: Vec<String> = snapshot
            .outbox
            .values()
            .map(|record| {
                let tag = match &record.state {
                    OutboxState::Pending { .. } => "pending",
                    OutboxState::Claimed(_) => "claimed",
                    OutboxState::Delivered { .. } => "delivered",
                    OutboxState::DeliveryUnknown { .. } => "unknown",
                    OutboxState::Quarantined { .. } => "quarantined",
                };
                format!("{}:{tag}", record.topic)
            })
            .collect();
        outbox.sort();
        out.push_str(&format!(" outbox=[{}]", outbox.join(",")));
    }
    out
}

/// Poll until the finished run's rows are gone, driving the kernel's sweeper
/// each tick, and assert no row family is left behind. A fault or a race that
/// left the run un-sweepable keeps the rows alive and fails the poll either
/// way.
pub(crate) async fn sweep_until_gone(engine: &Engine, pid: &str) {
    let store = engine.runtime().store();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    let what = format!("the finished run {pid}'s rows to be swept");
    poll_until_within(
        &what,
        || async {
            if store.procs().find(pid).await.is_err() {
                return true;
            }
            let _ = engine.runtime().sweep_removable().await;
            false
        },
        || dump_state(engine, pid),
        SWEEP_WAIT,
    )
    .await;
    assert!(
        store.procs().find(pid).await.is_err(),
        "the finished run's proc row must be removed"
    );
    assert!(
        store.tasks().query(&q).await.unwrap().rows.is_empty(),
        "the finished run's task rows must be removed"
    );
    let ops = store.ops().query(&q).await.unwrap().rows;
    assert!(
        ops.is_empty(),
        "the finished run's outbox rows must be removed, left: {ops:?}"
    );
    let vars = store.vars().query(&q).await.unwrap().rows;
    assert!(
        vars.is_empty(),
        "the finished run's vars rows must be removed, left: {vars:?}"
    );
    let deliveries = store.deliveries().query(&q).await.unwrap().rows;
    assert!(
        deliveries.is_empty(),
        "the finished run's delivery rows must be removed, left: {deliveries:?}"
    );
    let messages = store.messages().query(&q).await.unwrap().rows;
    assert!(
        messages.is_empty(),
        "the finished run's message rows must be removed, left: {messages:?}"
    );
}

/// Drain the write-behind queue: a test that reads projected rows must see
/// every queued write applied first.
pub(crate) async fn flush(engine: &Engine) {
    engine.runtime().flush_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// Messages and client actions
// ---------------------------------------------------------------------------

/// Signal that fires once the irq act keyed `key` reports `Created`, carrying
/// `(pid, tid)` — how a cell learns the act it wants to act on.
pub(crate) fn capture_created(engine: &Engine, key: &str) -> Signal<(String, String)> {
    let sig = engine.signal((String::new(), String::new()));
    let s2 = sig.clone();
    let key = key.to_string();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        let key = key.clone();
        async move {
            if e.is_params_key(&key) && e.is_state(MessageState::Created) {
                s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2.close();
            }
        }
    });
    sig
}

/// Signal that fires once a run reports completion, carrying its pid.
pub(crate) fn capture_complete(engine: &Engine) -> Signal<String> {
    let sig = engine.signal(String::new());
    let s2 = sig.clone();
    engine.channel().on_complete(move |e| {
        let s2 = s2.clone();
        async move {
            s2.update(|d| *d = e.pid.clone());
            s2.close();
        }
    });
    sig
}

/// Longest a derived act's dispatch is waited for: a bounded wait, so a wedged
/// fold fails the test instead of hanging the suite (and long enough for the
/// outbox pass that carries the message under a loaded machine).
pub(crate) const EXPECT_WAIT: Duration = Duration::from_secs(60);

/// The external id (`tid`) of one instance: the C5 wire of `node`'s
/// `generation` under `pid` — what messages carry and client actions address
/// (the node itself is the message's `nid`, unchanged by C5).
pub(crate) fn tid_of(pid: &str, node: &str, generation: u64) -> String {
    crate::scheduler::core::model::task_graph::TaskId::new(node, generation)
        .unwrap()
        .wire(pid)
}

/// The `(pid, tid)` a flow must dispatch, failing (never hanging) if it wedges
/// before it does. Registered *before* the action that triggers the act.
pub(crate) async fn expect_created(sig: &Signal<(String, String)>) -> (String, String) {
    tokio::time::timeout(EXPECT_WAIT, sig.recv())
        .await
        .expect("the act must be dispatched before the deadline")
}

/// Serve one client action on `tid` and answer its outcome.
pub(crate) async fn act(
    engine: &Engine,
    pid: &str,
    tid: &str,
    action: EventAction,
    options: Vars,
) -> Result<()> {
    engine
        .runtime()
        .do_action(&Action::new(pid, tid, action, options))
        .await
}

/// Complete `tid` (the wire `Next`).
pub(crate) async fn complete_act(engine: &Engine, pid: &str, tid: &str) -> Result<()> {
    act(engine, pid, tid, EventAction::Next, Vars::new()).await
}

/// Fail `tid` with `ecode`/`message` (the wire `Error`).
pub(crate) async fn fail_act(
    engine: &Engine,
    pid: &str,
    tid: &str,
    ecode: &str,
    message: &str,
) -> Result<()> {
    let options = Vars::new().with("ecode", ecode).with("message", message);
    act(engine, pid, tid, EventAction::Error, options).await
}

/// Abort `tid` (the wire `Abort`).
pub(crate) async fn abort_act(engine: &Engine, pid: &str, tid: &str) -> Result<()> {
    act(engine, pid, tid, EventAction::Abort, Vars::new()).await
}

/// Cancel `tid` (the wire `Cancel`).
pub(crate) async fn cancel_act(engine: &Engine, pid: &str, tid: &str) -> Result<()> {
    act(engine, pid, tid, EventAction::Cancel, Vars::new()).await
}

/// Back `tid` to `to` (the wire `Back`).
pub(crate) async fn back_act(engine: &Engine, pid: &str, tid: &str, to: &str) -> Result<()> {
    act(
        engine,
        pid,
        tid,
        EventAction::Back,
        Vars::new().with("to", to),
    )
    .await
}

/// Skip `tid` (the wire `Skip`).
pub(crate) async fn skip_act(engine: &Engine, pid: &str, tid: &str) -> Result<()> {
    act(engine, pid, tid, EventAction::Skip, Vars::new()).await
}

// ---------------------------------------------------------------------------
// The matrix's shared fixtures
// ---------------------------------------------------------------------------

/// A one-step workflow whose step runs the irq act keyed `key`: the smallest
/// workflow a client action can be aimed at. The step is built directly rather
/// than through [`Workflow::with_step`] because that takes a plain `fn` and the
/// key is the cell's own.
pub(crate) fn one_step_irq(key: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id("wf-one-step");
    workflow.steps = vec![
        crate::Step::new()
            .with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", key)),
    ];
    workflow
}

/// A two-step workflow whose steps each run one irq act (`act1`, then
/// `act2`): the smallest workflow where a `Cancel` has a running path ahead of
/// the completed step to undo, and a `Remove` can target a mid-flight act
/// without ending the process.
pub(crate) fn two_step_irq_workflow() -> Workflow {
    Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        })
}

/// The key of one run's authoritative aggregate document (the process segment
/// is hex-encoded; build it with this, never by hand).
pub(crate) fn process_key(pid: &str) -> String {
    crate::scheduler::durable::document_store::process_key(pid).expect("a pid encodes as a key")
}

/// The data key of one process's row in a collection (`<prefix>-id-<pid>`).
pub(crate) fn proc_data_key(prefix: &str, pid: &str) -> String {
    let sep = utils::consts::KEY_SEP;
    format!("{prefix}{sep}id{sep}{pid}")
}

/// The data key of one task's row in a collection
/// (`<prefix>-id-<pid><tid>`; see [`utils::Id::id`]).
pub(crate) fn task_data_key(prefix: &str, pid: &str, tid: &str) -> String {
    let sep = utils::consts::KEY_SEP;
    format!("{prefix}{sep}id{sep}{}", utils::Id::new(pid, tid).id())
}

// ---------------------------------------------------------------------------
// Fault backends
// ---------------------------------------------------------------------------

/// Which write a [`FaultKv`] arm means to break.
pub(crate) enum FaultArm {
    /// One exact document key.
    Key(String),
    /// Any document key of the family but the listed ones: the write of work
    /// the call under test creates, immune to a lagging write of a row that
    /// already exists.
    KeyButNot(Vec<String>),
    /// Every key of one family, by prefix (`"kernel/aggregates/"`,
    /// `format!("tasks-id-{pid}")` for every task row of one run).
    Prefix(String),
    /// Any write at all.
    Any,
}

impl FaultArm {
    fn matches(&self, key: &str) -> bool {
        match self {
            FaultArm::Key(k) => k == key,
            FaultArm::KeyButNot(known) => !known.iter().any(|k| k == key),
            FaultArm::Prefix(prefix) => key.starts_with(prefix),
            FaultArm::Any => true,
        }
    }
}

/// A store whose next matching write fails once while armed, then works
/// normally: the transient store fault an operation, an action or a row projection
/// can hit. The fault is one-shot by construction, and [`Self::fired`] /
/// [`Self::failed_key`] let a cell prove it landed on the write it meant to
/// break instead of passing vacuously.
pub(crate) struct FaultKv {
    inner: MemoryStore,
    armed: AtomicBool,
    fired: AtomicUsize,
    /// Which write the arm means to break; set together with `armed`.
    target: Mutex<Option<FaultArm>>,
    /// Data key of the write the fault actually hit.
    failed: Mutex<Option<String>>,
}

impl FaultKv {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
            fired: AtomicUsize::new(0),
            target: Mutex::new(None),
            failed: Mutex::new(None),
        })
    }

    /// This store as the engine sees it.
    pub(crate) fn store(self: &Arc<Self>) -> Arc<dyn KvStore> {
        self.clone()
    }

    pub(crate) fn arm(&self, arm: FaultArm) {
        *self.target.lock() = Some(arm);
        self.fired.store(0, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Fail the next write of one exact key.
    pub(crate) fn arm_key(&self, key: &str) {
        self.arm(FaultArm::Key(key.to_string()));
    }

    /// Fail the next write of one key family.
    pub(crate) fn arm_prefix(&self, prefix: impl Into<String>) {
        self.arm(FaultArm::Prefix(prefix.into()));
    }

    pub(crate) fn arm_any(&self) {
        self.arm(FaultArm::Any);
    }

    /// Disarm without firing.
    pub(crate) fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    /// How many times the injected fault fired (never more than one).
    pub(crate) fn fired(&self) -> usize {
        self.fired.load(Ordering::SeqCst)
    }

    /// The key the fault hit, if it fired.
    pub(crate) fn failed_key(&self) -> Option<String> {
        self.failed.lock().clone()
    }

    /// The matching key of this write, consuming the arm when there is one.
    fn hit(&self, keys: impl Iterator<Item = String>) -> Option<String> {
        if !self.armed.load(Ordering::SeqCst) {
            return None;
        }
        let hit = {
            let target = self.target.lock();
            let target = target.as_ref()?;
            keys.into_iter().find(|key| target.matches(key))?
        };
        // consume: exactly one failure, then the backend is whole again
        self.armed.store(false, Ordering::SeqCst);
        self.fired.fetch_add(1, Ordering::SeqCst);
        *self.failed.lock() = Some(hit.clone());
        Some(hit)
    }
}

fn injected(what: &str) -> ActError {
    ActError::Store(format!("injected: {what}"))
}

#[async_trait::async_trait]
impl KvStore for FaultKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        if self.hit(std::iter::once(key.to_string())).is_some() {
            return Err(injected("put failed"));
        }
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        if self.hit(std::iter::once(key.to_string())).is_some() {
            return Err(injected("delete failed"));
        }
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        let keys = ops.iter().map(|op| match op {
            StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => key.clone(),
        });
        if self.hit(keys).is_some() {
            return Err(injected("batch failed"));
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// A store that refuses every read and write while down: the outage an operation, a
/// timer pass or a sweep has to survive. Reads and writes are counted, so a
/// cell can prove the degraded path really attempted the store (and that the
/// engine came back to it once healed) instead of passing vacuously.
pub(crate) struct OutageKv {
    inner: MemoryStore,
    down: AtomicBool,
    reads: AtomicUsize,
    writes: AtomicUsize,
}

impl OutageKv {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            down: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        })
    }

    pub(crate) fn store(self: &Arc<Self>) -> Arc<dyn KvStore> {
        self.clone()
    }

    /// The store goes away.
    pub(crate) fn down(&self) {
        self.down.store(true, Ordering::SeqCst);
    }

    /// The store comes back.
    pub(crate) fn heal(&self) {
        self.down.store(false, Ordering::SeqCst);
    }

    pub(crate) fn is_down(&self) -> bool {
        self.down.load(Ordering::SeqCst)
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub(crate) fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    fn refused(&self, read: bool) -> Option<ActError> {
        if !self.down.load(Ordering::SeqCst) {
            return None;
        }
        if read {
            self.reads.fetch_add(1, Ordering::SeqCst);
        } else {
            self.writes.fetch_add(1, Ordering::SeqCst);
        }
        Some(injected("store is down"))
    }
}

#[async_trait::async_trait]
impl KvStore for OutageKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match self.refused(true) {
            Some(err) => Err(err),
            None => self.inner.one(key).await,
        }
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        match self.refused(true) {
            Some(err) => Err(err),
            None => self.inner.many(keys).await,
        }
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        match self.refused(false) {
            Some(err) => Err(err),
            None => self.inner.put(key, value).await,
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        match self.refused(false) {
            Some(err) => Err(err),
            None => self.inner.delete(key).await,
        }
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        match self.refused(false) {
            Some(err) => Err(err),
            None => self.inner.batch(ops, guards).await,
        }
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        match self.refused(true) {
            Some(err) => Err(err),
            None => self.inner.scan_prefix(key, options).await,
        }
    }
}

/// How long a gated read waits before it gives up and fails: a gate nobody
/// opens must fail the cell, never hang the suite.
const GATE_WAIT: Duration = Duration::from_secs(30);

/// A store that refuses every write from the `cut`-th one on: the crash a cell
/// stops the engine at an exact write boundary with, and the durable state a
/// later engine has to recover from. Reads keep working (the crash is of the
/// writer, not of the backend), and `writes` counts every write attempt so a
/// cell can find the boundaries it means to cut at.
pub(crate) struct CutKv {
    inner: MemoryStore,
    /// The 1-based index of the first refused write; `usize::MAX` while open.
    cut: AtomicUsize,
    writes: AtomicUsize,
    refused: AtomicUsize,
}

impl CutKv {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            cut: AtomicUsize::new(usize::MAX),
            writes: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
        })
    }

    pub(crate) fn store(self: &Arc<Self>) -> Arc<dyn KvStore> {
        self.clone()
    }

    /// Refuse every write from the `n`-th one (1-based) on.
    pub(crate) fn cut_at(&self, n: usize) {
        self.cut.store(n.max(1), Ordering::SeqCst);
    }

    /// The writer works again.
    pub(crate) fn heal(&self) {
        self.cut.store(usize::MAX, Ordering::SeqCst);
    }

    /// Every write attempt so far.
    pub(crate) fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    /// How many writes the cut refused.
    pub(crate) fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }

    fn blocked(&self) -> bool {
        let n = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= self.cut.load(Ordering::SeqCst) {
            self.refused.fetch_add(1, Ordering::SeqCst);
            return true;
        }
        false
    }
}

#[async_trait::async_trait]
impl KvStore for CutKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        if self.blocked() {
            return Err(injected("write cut"));
        }
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        if self.blocked() {
            return Err(injected("write cut"));
        }
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if self.blocked() {
            return Err(injected("write cut"));
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// A store that parks every read whose key starts with the gated prefix until
/// [`GateKv::release`] — the interleaving a cell needs to hold one reader while
/// another decides. `parks` counts the reads that had to wait, so a cell can
/// prove the gate was actually taken.
pub(crate) struct GateKv {
    inner: MemoryStore,
    gate: Mutex<Option<&'static str>>,
    parks: AtomicUsize,
    open: tokio::sync::Notify,
}

impl GateKv {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            gate: Mutex::new(None),
            parks: AtomicUsize::new(0),
            open: tokio::sync::Notify::new(),
        })
    }

    pub(crate) fn store(self: &Arc<Self>) -> Arc<dyn KvStore> {
        self.clone()
    }

    /// Park every read of a key under `prefix`.
    pub(crate) fn gate(&self, prefix: &'static str) {
        *self.gate.lock() = Some(prefix);
    }

    /// Let every parked reader through (and every later one).
    pub(crate) fn release(&self) {
        *self.gate.lock() = None;
        self.open.notify_waiters();
    }

    pub(crate) fn parks(&self) -> usize {
        self.parks.load(Ordering::SeqCst)
    }

    async fn pass(&self, key: &str) {
        let deadline = Instant::now() + GATE_WAIT;
        while self
            .gate
            .lock()
            .is_some_and(|prefix| key.starts_with(prefix))
        {
            self.parks.fetch_add(1, Ordering::SeqCst);
            assert!(
                Instant::now() < deadline,
                "the store gate was never released"
            );
            let _ = tokio::time::timeout(Duration::from_millis(10), self.open.notified()).await;
        }
    }
}

#[async_trait::async_trait]
impl KvStore for GateKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.pass(key).await;
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        for key in keys {
            self.pass(key).await;
        }
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        self.pass(key).await;
        self.inner.scan_prefix(key, options).await
    }
}
