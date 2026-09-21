//! Reliability-condition tests for the matrix: each feature row crossed
//! with store failure, restart, duplicate delivery, and concurrency.

mod abort;
mod action;
mod back;
mod cancel;
mod error;
mod next;
mod parallel;
mod remove;
mod stall;
mod subflow;
mod timeout;
mod timer;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde_json::json;

use crate::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Config, Engine,
    MessageState, Result, Signal, Vars, Workflow,
    scheduler::{Process, Runtime, Task},
    store::{
        KvStore, MemoryStore, ScanOptions, StoreBatchOp, StoreGuard,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::test::USES_IRQ,
};

/// Run a test body under a hard deadline. An engine stall — observed once as a
/// full-suite hang in the racing cells — must fail the test instead of freezing
/// the serial queue behind it.
async fn bounded<F: std::future::Future<Output = ()>>(what: &str, fut: F) {
    // Generous on purpose: a racing cell runs dozens of rounds, so the bound
    // must only catch a genuine stall (an engine deadlock), not a loaded
    // machine finishing its rounds slowly.
    tokio::time::timeout(Duration::from_secs(150), fut)
        .await
        .unwrap_or_else(|_| panic!("{what} stalled: no progress within 150s (engine stall?)"));
}

/// Longest any settle poll waits; every poll asserts its deadline instead of
/// letting a hang look like a pass.
const WAIT: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(20);

/// Poll `ready` until it holds or `WAIT` runs out. `what` names the awaited
/// condition: the deadline panic carries it, so a stall that only shows up
/// under CI's load says which condition starved instead of a bare deadline.
/// `ready` is an async closure so every tick can query live engine/store
/// state — the established deadline convention, never a bare sleep.
async fn poll_until<F, Fut>(what: &str, ready: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    poll_until_dumped(what, ready, || async { String::new() }).await
}

/// [`poll_until`] that also prints `dump()` once the wait passes a few
/// seconds (and again whenever the dumped state changes): a condition that
/// only stalls under CI's coverage run ships its live task/row evidence home
/// in the failure output, instead of leaving the next session to guess which
/// queue or row family ate the work. Keep the dump coarse — stable states,
/// no timestamps — so a genuinely stuck state prints once, not every tick.
async fn poll_until_dumped<F, Fut, D, DumpFut>(what: &str, mut ready: F, dump: D) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
    D: Fn() -> DumpFut,
    DumpFut: Future<Output = String>,
{
    let start = Instant::now();
    let deadline = start + WAIT;
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

/// `Ok`/`Err` of a raced action as a short printable outcome.
fn outcome(r: &crate::Result<()>) -> String {
    match r {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("err({e})"),
    }
}

/// KV backend that fails the next durable write of exactly one task's row
/// while armed, then passes everything through: the transient store fault a
/// `Cancel`/`Remove` state persist can hit. The fault lands on the one write
/// the action performs — and on nothing before or after it.
struct FailOnceTaskWriteKv {
    inner: MemoryStore,
    armed: AtomicBool,
    /// How many times the injected fault actually fired (never more than one).
    fired: AtomicUsize,
    /// Full data key of the row whose next write fails (`tasks-id-<tid>`).
    target: Mutex<Option<String>>,
}

impl FailOnceTaskWriteKv {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
            fired: AtomicUsize::new(0),
            target: Mutex::new(None),
        })
    }

    /// Arm the one-shot fault on `tid`'s task row. The row id is `{pid}{tid}`
    /// (see `utils::Id::id`), not the bare tid.
    fn arm(&self, pid: &str, tid: &str) {
        *self.target.lock() = Some(format!(
            "{}{}id{}{}",
            "tasks",
            crate::utils::consts::KEY_SEP,
            crate::utils::consts::KEY_SEP,
            utils::Id::new(pid, tid).id()
        ));
        self.fired.store(0, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// How many times the injected fault fired.
    fn fired(&self) -> usize {
        self.fired.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for FailOnceTaskWriteKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> crate::Result<bool> {
        let hit = self.armed.load(Ordering::SeqCst)
            && match self.target.lock().as_deref() {
                Some(target) => ops
                    .iter()
                    .any(|op| matches!(op, StoreBatchOp::Put { key, .. } if key == target)),
                None => false,
            };
        if hit {
            // consume: exactly one failure, then the backend is whole again
            self.armed.store(false, Ordering::SeqCst);
            self.fired.fetch_add(1, Ordering::SeqCst);
            return Err(ActError::Store(
                "injected: task row write failed".to_string(),
            ));
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// Signal that fires once the irq act keyed `key` reports `Created`, carrying
/// `(pid, tid)` — how a test learns the act task it wants to act on.
fn irq_created_signal(engine: &Engine, key: &'static str) -> Signal<(String, String)> {
    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        async move {
            if e.is_params_key(key) && e.is_state(MessageState::Created) {
                s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2.close();
            }
        }
    });
    s
}

/// A two-step workflow whose steps each run one irq act (`act1`, then
/// `act2`): the smallest workflow where `Cancel` has a running path ahead of
/// the completed step to undo, and `Remove` can target a mid-flight act
/// without ending the process.
fn two_step_irq_workflow() -> Workflow {
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

/// The irq act tasks keyed `key` of the process.
fn acts_of(proc: &Arc<Process>, key: &str) -> Vec<Arc<Task>> {
    proc.task_by_params("key", key)
}

/// The waiting irq act keyed `key` (`Interrupt`), if it is waiting.
fn waiting_act(proc: &Arc<Process>, key: &str) -> Option<Arc<Task>> {
    acts_of(proc, key)
        .into_iter()
        .find(|t| t.state().is_interrupted())
}

/// Poll until an irq act keyed `key` is waiting, and return it.
async fn wait_waiting_act(proc: &Arc<Process>, key: &str) -> Arc<Task> {
    let what = format!("act '{key}' to be waiting");
    poll_until(&what, || async { waiting_act(proc, key).is_some() }).await;
    waiting_act(proc, key).expect("the waiting act must still be there")
}

/// The process reached a business-success end and no task of it is left in a
/// non-terminal state: nothing was left half-decided by a fault or a race.
fn assert_settled(proc: &Arc<Process>) {
    assert!(
        proc.state().is_biz_success(),
        "process must settle in business success, got {}",
        proc.state()
    );
    let unfinished: Vec<String> = proc
        .tasks()
        .iter()
        .filter(|t| !t.state().is_completed())
        .map(|t| format!("{}:{}", t.node().id(), t.state()))
        .collect();
    assert!(
        unfinished.is_empty(),
        "completed process must have no non-terminal tasks: {unfinished:?}"
    );
}

/// The process's live task set as a sorted `(task id, state)` list — the
/// comparable snapshot of a decision point.
fn task_snapshot(proc: &Arc<Process>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = proc
        .tasks()
        .iter()
        .map(|t| (t.id.clone(), t.state().to_string()))
        .collect();
    v.sort();
    v
}

/// Wait until the task set stops moving: the same snapshot on three
/// consecutive ticks. An `Abort`/`Cancel` decision lands on the process's
/// scheduler lane and its ancestor walk runs asynchronously after
/// `do_action` returns, so a state comparison taken immediately would race
/// the walk (and anything the walk schedules).
async fn settle_tasks(proc: &Arc<Process>) {
    let deadline = Instant::now() + WAIT;
    let mut last = task_snapshot(proc);
    let mut stable = 0;
    loop {
        tokio::time::sleep(TICK).await;
        let now = task_snapshot(proc);
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

/// Poll until the finished process's rows are gone, driving the sweeper each
/// tick, and assert no row family is left behind. This is the single merge of
/// the two per-file copies: the same sweeper-driven poll the neighbouring
/// recovery tests use (one copy waited on this `poll_until` deadline, the other
/// on a bounded tick count) with the same three assertions on the leftovers.
/// Work a fault or a race left stuck keeps the rows alive and fails the poll
/// either way.
async fn sweep_until_gone(rt: &Runtime, pid: &str) {
    let store = rt.cache().store();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    poll_until("the finished process's proc row to be swept", || async {
        store.procs().find(pid).await.is_err() || {
            let _ = rt.cache().sweep_removable().await;
            false
        }
    })
    .await;
    assert!(
        store.procs().find(pid).await.is_err(),
        "the finished process's proc row must be removed"
    );
    assert!(
        store.tasks().query(&q).await.unwrap().rows.is_empty(),
        "the finished process's task rows must be removed"
    );
    let ops = store.ops().query(&q).await.unwrap().rows;
    assert!(
        ops.is_empty(),
        "the finished process's outbox rows must be removed, left: {ops:?}"
    );
}

/// KV backend whose task-lifecycle write fails exactly once while armed: the
/// transient store fault a `do_action(Next)` state persist can hit. The fault
/// is visible to the test through `injected`, so a run where the condition
/// never fired fails the test instead of passing vacuously.
struct FailOnceTaskPutKv {
    inner: MemoryStore,
    armed: AtomicBool,
    injected: AtomicUsize,
    /// Exact task-row data key whose write must fail while armed; `None`
    /// fails the next task-lifecycle write of any task.
    target: Mutex<Option<String>>,
    /// Data key of the row write the fault actually hit, so a test can prove
    /// the fault landed on the write it means to break.
    failed: Mutex<Option<String>>,
}

impl FailOnceTaskPutKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
            injected: AtomicUsize::new(0),
            target: Mutex::new(None),
            failed: Mutex::new(None),
        }
    }

    /// Fail the next task-lifecycle write of any task.
    fn arm(&self) {
        *self.target.lock() = None;
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Fail the next write of one task's own lifecycle row. The row id is
    /// `{pid}{tid}` (see `utils::Id::id`), not the bare tid.
    fn arm_row(&self, pid: &str, tid: &str) {
        *self.target.lock() = Some(task_data_key(pid, tid));
        self.armed.store(true, Ordering::SeqCst);
    }

    /// The data key of the row write the fault hit, if it fired.
    fn failed_key(&self) -> Option<String> {
        self.failed.lock().clone()
    }
}

#[async_trait::async_trait]
impl KvStore for FailOnceTaskPutKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> crate::Result<bool> {
        if self.armed.load(Ordering::SeqCst) {
            let target = self.target.lock().clone();
            let hit = ops.iter().find_map(|op| {
                let key = match op {
                    StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => key,
                };
                if !key.starts_with("tasks-id-") {
                    return None;
                }
                match target.as_deref() {
                    Some(target) if target != key => None,
                    _ => Some(key.clone()),
                }
            });
            if let Some(key) = hit {
                // consume: exactly one failure, then the backend is whole again
                self.armed.store(false, Ordering::SeqCst);
                self.injected.fetch_add(1, Ordering::SeqCst);
                *self.failed.lock() = Some(key);
                return Err(ActError::Store("injected: task put failed".to_string()));
            }
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// Register a handler that captures `(pid, tid)` of the first `Created`
/// message emitted for the act with the given params key.
fn capture_created(engine: &Engine, key: &str) -> Signal<(String, String)> {
    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
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
    s
}

fn task_row_id(pid: &str, tid: &str) -> String {
    utils::Id::new(pid, tid).id()
}

/// Full data key of a task's lifecycle row: `<iden>-id-<rowid>`.
fn task_data_key(pid: &str, tid: &str) -> String {
    let sep = utils::consts::KEY_SEP;
    format!("tasks{sep}id{sep}{}", task_row_id(pid, tid))
}

const SETTLE: Duration = Duration::from_millis(20);

/// Poll `ready` until it holds or `ticks` settle windows run out. Every caller
/// asserts the returned verdict, so a stuck engine fails the test instead of
/// passing vacuously.
async fn poll_settle<F, Fut>(mut ready: F, ticks: usize) -> bool
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

/// Longest a derived act's dispatch is waited for: a bounded wait, so a wedged
/// redo path fails the test instead of hanging the suite. (`WAIT` above is the
/// longer bound the settle polls use.)
const EXPECT_WAIT: Duration = Duration::from_secs(10);

/// The `(pid, tid)` a flow must dispatch, failing (never hanging) if it wedges
/// before it does. Registered *before* the action that triggers the act.
async fn expect_created(sig: &Signal<(String, String)>) -> (String, String) {
    tokio::time::timeout(EXPECT_WAIT, sig.recv())
        .await
        .expect("the act must be dispatched before the deadline")
}

/// Longest a probe is waited for; the assert inside the wait reports which
/// report never arrived.
const PROBE_WAIT: Duration = Duration::from_secs(10);

/// The env key the probe acts report through: the process env is the one
/// handle a package and its test share.
const PROBE: &str = "probe";

/// Reports "running", then waits for the act's cancellation before reporting
/// "stopped". Its counterpart in the other act reports the same two facts and
/// then fails, to pin what an overridden task does with the act's error.
#[derive(Debug, Clone)]
struct ProbePackage;

/// Same as [`ProbePackage`], but returns an error once it is cancelled.
#[derive(Debug, Clone)]
struct FailingProbePackage;

#[async_trait::async_trait]
impl ActPackage for ProbePackage {
    fn definition() -> ActPackageDefinition {
        definition("test.scheduler.probe")
    }

    fn new(_: &Config) -> Result<Self> {
        Ok(Self)
    }

    async fn execute(
        &self,
        ctx: &crate::Context,
        _params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        ctx.set_env(PROBE, "running");
        ctx.cancellation_token().cancelled().await;
        ctx.set_env(PROBE, "stopped");
        Ok(None)
    }
}

#[async_trait::async_trait]
impl ActPackage for FailingProbePackage {
    fn definition() -> ActPackageDefinition {
        definition("test.scheduler.failing_probe")
    }

    fn new(_: &Config) -> Result<Self> {
        Ok(Self)
    }

    async fn execute(
        &self,
        ctx: &crate::Context,
        _params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        ctx.set_env(PROBE, "running");
        ctx.cancellation_token().cancelled().await;
        ctx.set_env(PROBE, "stopped");
        // A package that treats the cancellation as its own failure
        Err(ActError::Runtime("act stopped early".to_string()))
    }
}

fn definition(id: &'static str) -> ActPackageDefinition {
    ActPackageDefinition {
        id,
        name: "Probe",
        desc: "report when the act's cancellation token fires",
        icon: "",
        doc: "",
        version: "0.1.0",
        schema: json!({}),
        options: None,
        run_as: ActRunAs::Func,
        resources: Vec::new(),
        catalog: ActPackageCatalog::App,
    }
}

/// Poll the process env until the probe reports `value`.
async fn wait_probe(proc: &Arc<Process>, value: &str) {
    let deadline = Instant::now() + PROBE_WAIT;
    loop {
        if proc.with_env(|env| env.get::<String>(PROBE)).as_deref() == Some(value) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the probe act never reported '{value}' (last: {:?})",
            proc.with_env(|env| env.get::<String>(PROBE))
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The act task of the probe, once it is in flight. The enclosing step node
/// carries the same `uses` (a step `uses` is how a single-act step is
/// written), so the kind is what tells the two apart.
fn running_act(proc: &Arc<Process>, uses: &str) -> Arc<Task> {
    proc.find_tasks(|task| {
        task.is_kind(crate::NodeKind::Act) && task.is_uses(uses) && task.state().is_running()
    })
    .into_iter()
    .next()
    .expect("the probe act must be running")
}
