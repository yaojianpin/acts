//! SUBFLOW reliability cells: the `acts.core.subflow` start — one run starting
//! another — under a store fault, a restart, a duplicated delivery and two
//! concurrent starts.
//!
//! What the kernel's subflow *is* shapes every assertion here. The subflow step
//! is one `Func` act: its effect resolves the child model, starts it
//! (`start_as_owner`, carrying the parent's authority and its
//! `$parent_pid`/`$parent_tid` inputs) and then completes, so the act and the
//! child's start are one operation. The child is a process of its own, with its own
//! aggregate, driven by its own operations.
//!
//! The kernel has **no parent↔child hand-back**: `return_to_act` was deleted
//! with the legacy engine, so nothing delivers a child's completion back to the
//! parent act and the parent waits for nothing. That is a documented boundary,
//! not an accident — `KERNEL_CUTOVER_NOTES.md` §“已知边界(本轮新增,未静默降级)”
//! records it, and the package's own `execute`
//! (`crates/acts/src/package/core/subflow.rs`) carries the same NOTE: the
//! subflow act now completes together with its child run, and the parent/child
//! inputs still reach the child but have no consumer. None of the cells here
//! therefore require the parent to wait for the child or to be completed by a
//! hand-back; the legacy hand-back intents are re-stated as the exactly-once
//! properties the kernel does hold.
//!
//! So the cells pin, on the surfaces the kernel has:
//!
//! - a store fault on the child's own start write reaches the subflow effect:
//!   the parent degrades to a defined terminal error instead of hanging, the
//!   refused start is atomic (no aggregate, no row, no announcement — no
//!   half-created child), and a fresh parent run completes exactly once and
//!   starts exactly one child, which completes exactly once;
//! - a restart between the child's start and its completion recovers both runs
//!   exactly once: the parent's subflow act is already terminal with its child
//!   start, so the recovered parent is *not* re-driven into a second child,
//!   while the child recovers with its act still waiting and settles exactly
//!   once on the new engine — exactly one child per subflow act across a crash;
//! - a duplicated delivery of the handoff act (the completion the handoff
//!   already applied, and a re-issued child completion) starts exactly one child
//!   process and completes each run exactly once, never two;
//! - two subflow acts of one parent start two children concurrently; each starts
//!   exactly one child, both complete exactly once, and the parent completes
//!   exactly once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;
use serial_test::serial;

use crate::scheduler::core::model::task_graph::TaskState;
use crate::store::ScanOperation;
use crate::utils::consts;
use crate::{LegacyTaskState, Step};

use super::*;

/// The parent model id.
const PARENT: &str = "main";
/// The child model id — the `to` of every subflow step here.
const CHILD: &str = "w2";
/// The parent's subflow step: the node a hand-back would act on.
const PARENT_NODE: &str = "step1";
/// The child's single irq step, and the node id its messages name.
const CHILD_NODE: &str = "s1";
/// The irq key the child's act announces under.
const ACT_KEY: &str = "act1";
/// The subflow package id.
const SUBFLOW: &str = "acts.core.subflow";
/// The aggregate-key family the harness's [`process_key`] builds. Private in
/// `document_store`, so restated: it is the one write family that *is* the
/// durable run.
const PROCESS_PREFIX: &str = "kernel/aggregates/";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The parent workflow: one subflow step calling the child model `mid`. The
/// step's `uses` is the whole node, so the subflow act and the step are the
/// same execution.
fn parent_flow(mid: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(PARENT);
    workflow.steps = vec![subflow_step(PARENT_NODE, mid)];
    workflow
}

/// The parent workflow of the concurrency cell: one step with two branches,
/// each running a subflow step — two children of one parent, in flight at
/// once.
fn branched_parent_flow() -> Workflow {
    let mut workflow = Workflow::new().with_id(PARENT);
    workflow.steps = vec![
        Step::new()
            .with_id(PARENT_NODE)
            .with_branch(|branch| {
                branch.with_id("b1").with_if("true").with_step(|step| {
                    step.with_id("s1")
                        .with_uses(SUBFLOW, Vars::from(json!({ "to": CHILD })))
                })
            })
            .with_branch(|branch| {
                branch.with_id("b2").with_if("true").with_step(|step| {
                    step.with_id("s2")
                        .with_uses(SUBFLOW, Vars::from(json!({ "to": CHILD })))
                })
            }),
    ];
    workflow
}

fn subflow_step(id: &str, mid: &str) -> Step {
    Step::new()
        .with_id(id)
        .with_uses(SUBFLOW, Vars::from(json!({ "to": mid })))
}

/// The child workflow: one irq act an external client must complete.
fn child_flow() -> Workflow {
    let mut workflow = Workflow::new().with_id(CHILD);
    workflow.steps = vec![
        Step::new()
            .with_id(CHILD_NODE)
            .with_uses(USES_IRQ, Vars::new().with("key", ACT_KEY)),
    ];
    workflow
}

/// Deploy the child model: `start_as_owner` resolves the child by its `mid`, so
/// the subflow can only start a model the store holds.
async fn deploy(engine: &Engine, workflow: &Workflow) {
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(workflow, None)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The aggregate's state for one model node of one run, if the aggregate is
/// still there — the durable truth, immune to the read model's lag and to the
/// sweeper.
async fn legacy_task_state(engine: &Engine, pid: &str, node: &str) -> Option<TaskState> {
    let snapshot = engine.runtime().kernel().snapshot(pid).await.ok()??;
    snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node.as_str() == node)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, task)| task.state)
}

/// How many live runs of the child model the store holds. A process is one row
/// of its own here (unlike the branches of one node id inside one run, a
/// subflow child is a *separate* run), so `mid` is what a child census counts.
async fn child_runs(engine: &Engine) -> usize {
    let query = Query::new().filter(Filter::and().expr(Expr::eq("mid", CHILD)));
    engine
        .runtime()
        .store()
        .procs()
        .query(&query)
        .await
        .unwrap()
        .rows
        .len()
}

/// Count the completions the runs of one model report on the engine's channel.
/// A subflow cell runs two models, so the harness's [`capture_complete`] (which
/// fires on the first completion of *either*) cannot count one of them, and an
/// aggregate-level wait cannot either.
fn count_completions_of(engine: &Engine, mid: &str) -> Arc<AtomicUsize> {
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    let mid = mid.to_string();
    engine.channel().on_complete(move |e| {
        let c = c.clone();
        let mid = mid.clone();
        async move {
            if e.mid == mid {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    counter
}

/// Count the terminal events — a completion or an error — the runs of one
/// model report on the engine's channel. A run that degrades ends in an error
/// event, not a completion, so a cell that pins "settles exactly once" for a
/// run whose end may be either must count both.
fn count_terminals_of(engine: &Engine, mid: &str) -> Arc<AtomicUsize> {
    let counter = Arc::new(AtomicUsize::new(0));
    let mid = mid.to_string();
    let c = counter.clone();
    let watched = mid.clone();
    engine.channel().on_complete(move |e| {
        let c = c.clone();
        let watched = watched.clone();
        async move {
            if e.mid == watched {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let c = counter.clone();
    engine.channel().on_error(move |e| {
        let c = c.clone();
        let mid = mid.clone();
        async move {
            if e.mid == mid {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    counter
}

/// Wait until `counter` reaches `n`, then assert it holds exactly `n`. A count
/// of the engine's own events needs this: the events are delivered
/// asynchronously, so an aggregate-level wait (which reads the durable truth)
/// does not order against the event stream.
async fn expect_count(counter: &Arc<AtomicUsize>, n: usize, what: &str) {
    poll_until(what, || async { counter.load(Ordering::SeqCst) >= n }).await;
    assert_eq!(counter.load(Ordering::SeqCst), n, "{what}");
}

/// Every `Created` announcement of the act keyed `key`, as it arrives: the
/// child-start census. The messages name the child's own pid, so the census
/// counts starts the read model could not tell apart from a shared row.
fn collect_created(engine: &Engine, key: &str) -> Arc<Mutex<Vec<(String, String)>>> {
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let rec = seen.clone();
    let key = key.to_string();
    engine.channel().on_message(move |e| {
        let rec = rec.clone();
        let key = key.clone();
        async move {
            if e.is_params_key(&key) && e.is_state(MessageState::Created) {
                rec.lock().push((e.pid.clone(), e.tid.clone()));
            }
        }
    });
    seen
}

/// The first `n` `Created` announcements of the act keyed `key`, as a signal
/// that closes on the `n`-th: how the concurrency cell learns both children
/// without racing a count.
fn created_signal(engine: &Engine, key: &str, n: usize) -> Signal<Vec<(String, String)>> {
    let sig = engine.signal(Vec::<(String, String)>::new());
    let s2 = sig.clone();
    let key = key.to_string();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        let key = key.clone();
        async move {
            if e.is_params_key(&key) && e.is_state(MessageState::Created) {
                s2.update(|seen| {
                    seen.push((e.pid.clone(), e.tid.clone()));
                    if seen.len() >= n {
                        s2.close();
                    }
                });
            }
        }
    });
    sig
}

/// The `n` children a flow must dispatch, failing (never hanging) if it wedges
/// before it does.
async fn expect_children(sig: &Signal<Vec<(String, String)>>, n: usize) -> Vec<(String, String)> {
    tokio::time::timeout(EXPECT_WAIT, sig.recv())
        .await
        .unwrap_or_else(|_| panic!("{n} subflow children must be dispatched before the deadline"))
}

/// Assert no further `Created` of the act is announced within a bounded window:
/// a refused or recovered path must not start another child at all.
async fn assert_no_created(sig: &Signal<(String, String)>) {
    let announced = tokio::time::timeout(Duration::from_millis(2000), sig.recv()).await;
    assert!(
        announced.is_err(),
        "no further child may be started: {:?}",
        announced.ok()
    );
}

// ---------------------------------------------------------------------------
// SUBFLOW × DB失败 — a fault on the child's own start write
// ---------------------------------------------------------------------------

/// The store every cell here runs on: a `MemoryStore` that watches the starts
/// of runs — the first write of a run's aggregate key — so a cell can count how
/// many runs were created and prove a child was never duplicated. The census is
/// the durable one the row model cannot give: a run's *start* has no row until
/// it is projected, and the aggregate document is the run's own record from its
/// first instant.
///
/// The fault cell arms it: the first run start that is not one the cell allowed
/// is refused exactly once. The runs the cell starts itself are allowed by their
/// aggregate key up front — the cell starts the parent under an external pid, so
/// the key is known before the run exists — so the parent's create and every
/// later commit of its aggregate pass untouched, and the first start of any
/// *other* run is the one refused.
///
/// The kernel creates a run as one transaction (the aggregate plus its recovery
/// and outbox indexes), so the refusal is atomic: the child has no aggregate,
/// no index entry and no row to half-exist.
///
/// This is the kernel's nearest write to the legacy fault cell's: the legacy
/// store failed the *second* `procs-id-` row batch (the child's process row),
/// but a row is a projection here — a failed row write is logged and dropped,
/// never propagated into the operation — so the start's durable unit, the aggregate
/// transaction, is what a fault has to break to reach the subflow effect.
struct SubflowKv {
    inner: MemoryStore,
    /// The aggregate keys that must pass: the runs the cell starts itself.
    allowed: Mutex<Vec<String>>,
    armed: AtomicBool,
    fired: AtomicUsize,
    failed: Mutex<Option<String>>,
    /// The distinct *other* runs whose start write the store has seen. A run's
    /// own later commits rewrite the same aggregate key, so only the first
    /// sight of a key is a start.
    starts: Mutex<Vec<String>>,
}

impl SubflowKv {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            allowed: Mutex::new(Vec::new()),
            armed: AtomicBool::new(false),
            fired: AtomicUsize::new(0),
            failed: Mutex::new(None),
            starts: Mutex::new(Vec::new()),
        })
    }

    fn store(self: &Arc<Self>) -> Arc<dyn KvStore> {
        self.clone()
    }

    /// Refuse the next run start that is not `parent_pid`'s.
    fn arm(&self, parent_pid: &str) {
        self.allowed.lock().push(process_key(parent_pid));
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Let `parent_pid`'s run through without arming anything: the next cell
    /// section starts a run of its own.
    fn allow(&self, parent_pid: &str) {
        self.allowed.lock().push(process_key(parent_pid));
        self.armed.store(false, Ordering::SeqCst);
    }

    fn fired(&self) -> usize {
        self.fired.load(Ordering::SeqCst)
    }

    /// How many distinct other runs the store has seen started.
    fn starts(&self) -> usize {
        self.starts.lock().len()
    }

    fn failed_key(&self) -> Option<String> {
        self.failed.lock().clone()
    }

    /// Every aggregate the store holds, sorted: the run-level census that shows
    /// whether a child half-exists.
    async fn processes(&self) -> Vec<String> {
        let options = ScanOptions::new(ScanOperation::Eq, PROCESS_PREFIX.to_string(), false);
        let mut keys: Vec<String> = self
            .inner
            .scan_prefix(PROCESS_PREFIX, options)
            .await
            .unwrap()
            .into_iter()
            .map(|(key, _)| key)
            // The base and log entries live under the head's prefix; only the
            // head names a run.
            .filter(|key| crate::scheduler::durable::document_store::is_process_head_key(key))
            .collect();
        keys.sort();
        keys
    }

    /// The *new* run's aggregate key of this write, consuming the arm. `None`
    /// for a run the cell allowed, for a later commit of a run already seen,
    /// and for writes that create no run.
    fn hit(&self, keys: impl Iterator<Item = String>) -> Option<String> {
        let foreign = {
            let allowed = self.allowed.lock();
            if allowed.is_empty() {
                return None;
            }
            keys.into_iter().find(|key| {
                // The base and log entries share the prefix; only a head is a
                // run start.
                crate::scheduler::durable::document_store::is_process_head_key(key)
                    && !allowed.contains(key)
            })?
        };
        {
            let mut starts = self.starts.lock();
            if starts.iter().any(|key| key == &foreign) {
                return None;
            }
            starts.push(foreign.clone());
        }
        if !self.armed.load(Ordering::SeqCst) {
            return None;
        }
        self.armed.store(false, Ordering::SeqCst);
        self.fired.fetch_add(1, Ordering::SeqCst);
        *self.failed.lock() = Some(foreign.clone());
        Some(foreign)
    }
}

#[async_trait::async_trait]
impl KvStore for SubflowKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        if self.hit(std::iter::once(key.to_string())).is_some() {
            return Err(ActError::Store("injected: child start refused".to_string()));
        }
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        let keys = ops.iter().map(|op| match op {
            StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => key.clone(),
        });
        if self.hit(keys).is_some() {
            return Err(ActError::Store("injected: child start refused".to_string()));
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// A store refusal on the child's own start write must reach the subflow
/// effect, and the effect's failure takes the failure path: the parent settles
/// in a defined terminal error instead of hanging mid-handoff. The refused
/// start is atomic — the child's `create` is one transaction of aggregate plus
/// indexes — so no half-created child survives: no second aggregate, no child
/// row, and no `Created` announcement.
///
/// The fault is one-shot, so the healed engine proves its own health: a fresh
/// parent completes exactly once at its handoff and starts exactly one child,
/// which completes exactly once when the client completes it. (The parent's
/// completion is the handoff's own — see the module note: the kernel's subflow
/// act completes with the child's start.)
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_store_that_refuses_the_child_start_degrades_the_parent_and_heals() {
    bounded(
        "sch_subflow_store_that_refuses_the_child_start_degrades_the_parent_and_heals",
        a_store_that_refuses_the_child_start_degrades_the_parent_and_heals_inner(),
    )
    .await;
}

async fn a_store_that_refuses_the_child_start_degrades_the_parent_and_heals_inner() {
    let kv = SubflowKv::new();
    let store = kv.store();
    let engine = engine_on(store).await;
    deploy(&engine, &child_flow()).await;

    // The parent runs under an external pid: its aggregate key is what the
    // refusal must *not* touch, and naming it up front is what makes the fault
    // land on the child's start instead of the parent's own create.
    kv.arm("subflowfaulted");
    let created = capture_created(&engine, ACT_KEY);
    let parent_done = count_terminals_of(&engine, PARENT);
    let parent_completed = count_completions_of(&engine, PARENT);
    let vars = Vars::new().with(consts::PROCESS_ID, "subflowfaulted");
    let pid = start_run_with(&engine, &parent_flow(CHILD), vars).await;
    assert_eq!(
        pid, "subflowfaulted",
        "the run carries the pid it was given"
    );

    // the refused durable write reaches the effect: the parent errors, no hang
    wait_completed(&engine, &pid).await;
    assert_eq!(kv.fired(), 1, "the armed refusal must fire exactly once");
    let refused = kv.failed_key().expect("the refusal must name its key");
    assert!(
        refused.starts_with(PROCESS_PREFIX),
        "the fault must land on an aggregate write, got {refused}"
    );
    assert_ne!(
        refused,
        process_key(&pid),
        "the fault must land on the child's start, never on the parent's own create"
    );
    assert_eq!(
        kv.starts(),
        1,
        "the child start must have been attempted exactly once, never replayed"
    );
    assert_eq!(
        legacy_task_state(&engine, &pid, PARENT_NODE).await,
        Some(TaskState::Failed),
        "the subflow step must degrade to a defined terminal error"
    );
    // no half-created child: the refused start wrote nothing at all
    assert_eq!(
        kv.processes().await,
        vec![process_key(&pid)],
        "a refused child start must leave no child aggregate behind"
    );
    flush(&engine).await;
    assert_eq!(
        child_runs(&engine).await,
        0,
        "a refused child start must leave no child run row"
    );
    assert_no_created(&created).await;
    expect_count(
        &parent_done,
        1,
        "the faulted parent reports exactly one terminal event",
    )
    .await;
    assert_eq!(
        parent_completed.load(Ordering::SeqCst),
        0,
        "a faulted parent degrades to an error, never to a completion"
    );

    // healed: the refusal is spent, a fresh parent completes exactly once and
    // starts exactly one child
    kv.allow("subflowhealed");
    let healed_parent = count_completions_of(&engine, PARENT);
    let child_done = count_completions_of(&engine, CHILD);
    let child = capture_created(&engine, ACT_KEY);
    let vars = Vars::new().with(consts::PROCESS_ID, "subflowhealed");
    let pid2 = start_run_with(&engine, &parent_flow(CHILD), vars).await;
    let (child_pid, tid) = expect_created(&child).await;
    assert_ne!(child_pid, pid2, "the child is a run of its own");
    assert_eq!(
        tid,
        tid_of(&child_pid, CHILD_NODE, 0),
        "the child's act is its step's own instance"
    );
    wait_completed(&engine, &pid2).await;
    assert_eq!(
        legacy_task_state(&engine, &pid2, PARENT_NODE).await,
        Some(TaskState::Completed),
        "the healed parent's subflow act completes with the child's start"
    );
    assert!(
        poll_until("exactly one child run", || async {
            child_runs(&engine).await == 1
        })
        .await,
        "the healed run must start exactly one child"
    );
    complete_act(&engine, &child_pid, &tid).await.unwrap();
    wait_completed(&engine, &child_pid).await;
    expect_count(&child_done, 1, "the child completes exactly once").await;
    expect_count(
        &healed_parent,
        1,
        "the healed parent completes exactly once",
    )
    .await;
    assert_eq!(
        kv.starts(),
        2,
        "exactly one child start per parent run: one refused, one started"
    );
    assert_eq!(kv.fired(), 1, "the fault never fired again");

    sweep_until_gone(&engine, &pid).await;
    sweep_until_gone(&engine, &pid2).await;
    sweep_until_gone(&engine, &child_pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// SUBFLOW × 重启 — a restart between the child's start and its completion
// ---------------------------------------------------------------------------

/// A restart between the child's start and its completion must recover both
/// runs exactly once. The parent's handoff already committed (its subflow act
/// completed with the child's start), so the recovered parent is *not* re-run:
/// its effect never starts a second child, and no fresh `Created` is announced.
/// The child recovers as the live run it was, its act still waiting, and
/// completing it on the new engine completes it exactly once — the child's
/// completion is never delivered back to the parent (the kernel's documented
/// boundary: there is no hand-back channel), so the parent stays the completed
/// run it was and is never resurrected, duplicated or re-driven.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_restart_between_the_child_start_and_its_completion_recovers_both_once() {
    bounded(
        "sch_subflow_restart_between_the_child_start_and_its_completion_recovers_both_once",
        a_restart_between_the_child_start_and_its_completion_recovers_both_once_inner(),
    )
    .await;
}

async fn a_restart_between_the_child_start_and_its_completion_recovers_both_once_inner() {
    let kv = SubflowKv::new();
    let store = kv.store();
    let engine = engine_on(store.clone()).await;
    deploy(&engine, &child_flow()).await;

    let created = capture_created(&engine, ACT_KEY);
    let census = collect_created(&engine, ACT_KEY);
    let parent_done = count_completions_of(&engine, PARENT);
    let pid = start_run(&engine, &parent_flow(CHILD)).await;
    let (child_pid, tid) = expect_created(&created).await;
    assert_ne!(child_pid, pid, "the child is a run of its own");
    assert_eq!(
        tid,
        tid_of(&child_pid, CHILD_NODE, 0),
        "the child's act is its step's own instance"
    );

    // the handoff is where the parent ends: the child is in flight, waiting
    wait_completed(&engine, &pid).await;
    wait_waiting_task(&engine, &child_pid, CHILD_NODE).await;
    assert!(
        poll_until("exactly one child run", || async {
            child_runs(&engine).await == 1
        })
        .await,
        "exactly one child run before the restart"
    );

    // crash and reload over the same store
    let engine = restart(&engine, store.clone()).await;

    // the child recovered exactly as it was: one waiting act, one run
    let child = wait_waiting_task(&engine, &child_pid, CHILD_NODE).await;
    assert_eq!(
        child.state(),
        LegacyTaskState::Interrupt,
        "the child's act must recover as the waiting act it was"
    );
    assert!(
        poll_until("exactly one child run after the restart", || async {
            child_runs(&engine).await == 1
        })
        .await,
        "recovery must not start a second child"
    );
    // the parent is not re-run: its aggregate still holds the completed
    // handoff, and the boot never announces a fresh child
    assert!(
        engine
            .runtime()
            .kernel()
            .process_completed(&pid)
            .await
            .unwrap_or(false),
        "the parent must recover as the completed run it was"
    );
    assert_eq!(
        legacy_task_state(&engine, &pid, PARENT_NODE).await,
        Some(TaskState::Completed),
        "the recovered parent's subflow act stays the completed handoff"
    );
    let extra = capture_created(&engine, ACT_KEY);
    assert_no_created(&extra).await;
    expect_count(
        &parent_done,
        1,
        "the parent completed exactly once, before the restart",
    )
    .await;

    // completing the child on the new engine completes it exactly once; the
    // parent is not driven by it
    let child_done = count_completions_of(&engine, CHILD);
    complete_act(&engine, &child_pid, &tid).await.unwrap();
    wait_completed(&engine, &child_pid).await;
    expect_count(
        &child_done,
        1,
        "the child completes exactly once across the restart",
    )
    .await;
    assert!(
        engine
            .runtime()
            .kernel()
            .process_completed(&pid)
            .await
            .unwrap_or(false),
        "the parent is not resurrected by the child's completion"
    );
    let announced = census.lock().clone();
    assert_eq!(
        announced.len(),
        1,
        "no second child was ever announced across the restart: {announced:?}"
    );
    assert_eq!(
        kv.processes().await.len(),
        2,
        "one aggregate per run across the restart — the parent and its one child"
    );

    sweep_until_gone(&engine, &child_pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// SUBFLOW × 重复投递 — a duplicated child completion is refused
// ---------------------------------------------------------------------------

/// A duplicated delivery must be refused with no second side effect. The child's
/// first completion is the client's own `Next`; the duplicate of it is refused
/// by the decision guard (the act is already terminal), and a completion aimed
/// at the parent's subflow act — the act the handoff already applied, since the
/// kernel has no hand-back channel to redeliver through — is refused too.
/// Neither starts a second child: exactly one child process is ever announced,
/// exactly one child aggregate exists, and the parent completed exactly once
/// (at the handoff).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_duplicate_child_completion_is_refused_and_starts_no_second_child() {
    bounded(
        "sch_subflow_duplicate_child_completion_is_refused_and_starts_no_second_child",
        a_duplicate_child_completion_is_refused_and_starts_no_second_child_inner(),
    )
    .await;
}

async fn a_duplicate_child_completion_is_refused_and_starts_no_second_child_inner() {
    let kv = SubflowKv::new();
    let engine = engine_on(kv.store()).await;
    deploy(&engine, &child_flow()).await;

    let created = collect_created(&engine, ACT_KEY);
    let first = capture_created(&engine, ACT_KEY);
    let parent_done = count_completions_of(&engine, PARENT);
    let child_done = count_completions_of(&engine, CHILD);
    let pid = start_run(&engine, &parent_flow(CHILD)).await;

    // the handoff: the parent is done exactly once, the child waits
    wait_completed(&engine, &pid).await;
    let (child_pid, tid) = expect_created(&first).await;
    assert_ne!(child_pid, pid, "the child is a run of its own");
    wait_waiting_task(&engine, &child_pid, CHILD_NODE).await;
    assert!(
        poll_until("exactly one child run", || async {
            child_runs(&engine).await == 1
        })
        .await,
        "the handoff starts exactly one child"
    );

    // the first delivery: the client completes the child
    complete_act(&engine, &child_pid, &tid).await.unwrap();
    wait_completed(&engine, &child_pid).await;

    // duplicate #1 — the same child completion re-issued is refused
    let second = complete_act(&engine, &child_pid, &tid).await;
    assert!(
        second.is_err(),
        "a duplicate child completion must be refused, got {second:?}"
    );
    // duplicate #2 — a completion of the parent's subflow act, completed by the
    // handoff, is refused as well
    let redelivered = complete_act(&engine, &pid, PARENT_NODE).await;
    assert!(
        redelivered.is_err(),
        "a completion of the handoff act must be refused, got {redelivered:?}"
    );

    // exactly once: one child start, one child completion, one parent completion
    assert_no_created(&capture_created(&engine, ACT_KEY)).await;
    let announced = created.lock().clone();
    assert_eq!(
        announced.len(),
        1,
        "exactly one child was ever announced: {announced:?}"
    );
    let child_pids: std::collections::HashSet<&String> =
        announced.iter().map(|(pid, _)| pid).collect();
    assert_eq!(
        child_pids,
        std::collections::HashSet::from([&child_pid]),
        "the subflow act started exactly one child process, never a second"
    );
    assert_eq!(
        kv.processes().await.len(),
        2,
        "one aggregate per run: the parent and its one child"
    );
    expect_count(&child_done, 1, "the child completes exactly once").await;
    expect_count(&parent_done, 1, "the parent completes exactly once").await;

    sweep_until_gone(&engine, &child_pid).await;
    sweep_until_gone(&engine, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// SUBFLOW × 并发 — two branches start two children
// ---------------------------------------------------------------------------

/// Two branches of one parent start two children concurrently: both children
/// are in flight at once (two child runs, two `Created` announcements of
/// distinct pids), completing them concurrently completes each exactly once, and
/// the parent — whose branches joined at the two starts — completes exactly
/// once.
///
/// The container is the kernel's own: a step's model branches
/// (`Step::with_branch`), each holding a subflow step. The legacy cell used the
/// `acts.core.parallel` package's `in`/`acts` expansion, which the kernel does
/// not drive from the model — `ctx.build_acts` wrote into the headless effect
/// scope only, no durable child exists — so the nearest kernel property is a
/// branched container, the shape the kernel's fan-out/fan-in actually runs.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_two_subflow_branches_start_two_children_and_the_parent_completes_once() {
    bounded(
        "sch_subflow_two_subflow_branches_start_two_children_and_the_parent_completes_once",
        two_subflow_branches_start_two_children_and_the_parent_completes_once_inner(),
    )
    .await;
}

async fn two_subflow_branches_start_two_children_and_the_parent_completes_once_inner() {
    let kv = SubflowKv::new();
    let engine = engine_on(kv.store()).await;
    deploy(&engine, &child_flow()).await;

    let children = created_signal(&engine, ACT_KEY, 2);
    let census = collect_created(&engine, ACT_KEY);
    let parent_done = count_completions_of(&engine, PARENT);
    let pid = start_run(&engine, &branched_parent_flow()).await;
    let started = expect_children(&children, 2).await;
    let pids: std::collections::HashSet<&String> = started.iter().map(|(pid, _)| pid).collect();
    assert_eq!(started.len(), 2, "two subflow branches, two children");
    assert_eq!(pids.len(), 2, "the branches are distinct child runs");
    assert!(
        !pids.contains(&pid),
        "neither child is the parent run: {started:?}"
    );
    assert!(
        started
            .iter()
            .all(|(child_pid, tid)| *tid == tid_of(child_pid, CHILD_NODE, 0)),
        "each child's act is its own step's instance: {started:?}"
    );

    // both children are in flight at the same time, and the parent is done
    assert!(
        poll_until("both children in flight", || async {
            child_runs(&engine).await == 2
        })
        .await,
        "both children must be live at once"
    );
    wait_completed(&engine, &pid).await;
    expect_count(
        &parent_done,
        1,
        "the parent completes exactly once, after its branches joined",
    )
    .await;

    // complete both children concurrently
    let (left, right) = (started[0].clone(), started[1].clone());
    let children_done = count_completions_of(&engine, CHILD);
    let (first, second) = tokio::join!(
        complete_act(&engine, &left.0, &left.1),
        complete_act(&engine, &right.0, &right.1),
    );
    first.expect("each child completes exactly once");
    second.expect("each child completes exactly once");
    wait_completed(&engine, &left.0).await;
    wait_completed(&engine, &right.0).await;
    expect_count(&children_done, 2, "each child completes exactly once").await;
    assert_eq!(
        parent_done.load(Ordering::SeqCst),
        1,
        "the children's completions never re-drive the parent"
    );
    assert_no_created(&capture_created(&engine, ACT_KEY)).await;
    let announced = census.lock().clone();
    assert_eq!(
        announced.len(),
        2,
        "no third child was ever announced: {announced:?}"
    );
    assert_eq!(
        kv.processes().await.len(),
        3,
        "one aggregate per run: the parent and exactly its two children"
    );

    sweep_until_gone(&engine, &pid).await;
    sweep_until_gone(&engine, &left.0).await;
    sweep_until_gone(&engine, &right.0).await;
    engine.close().await;
}
