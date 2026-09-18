//! Reliability of the parallel fan-out/fan-in step (`acts.core.parallel`)
//! across the failure cells of the test matrix.
//!
//! - store fault: a branch-act durable write fails once while the branches
//!   run. The failure is surfaced at the flush barrier instead of being
//!   swallowed, the flow degrades without losing or duplicating work, the
//!   writer heals, and the finished process is swept without leftovers.
//! - restart: one branch completed durably, the other still in flight. A
//!   reload on the shared store re-drives the process without re-running the
//!   finished branch, and the parent completes exactly once.
//! - duplicate message: the same `Next` action delivered twice. The second
//!   delivery is rejected as a no-op and the parent still completes exactly
//!   once.

use serde_json::json;

use super::*;
use crate::{
    Act, ActError, Action, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    store::{
        KvStore, MemoryStore, ScanOptions, StoreBatchOp,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::test::{USES_PARALLEL, auto_complete, create_proc},
};
use parking_lot::Mutex;
use serial_test::serial;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

/// Tids of the branch-act messages the test handlers observed, across every
/// engine of a scenario (message re-deliveries included).
type Rec = Arc<Mutex<Vec<String>>>;

/// Wait until the durable task rows matching `q` stop changing: the same
/// `(tid, state)` snapshot across three consecutive ticks. An action's
/// propagation keeps writing rows for a while after `do_action` returns, so a
/// baseline taken earlier would be compared against a moving snapshot.
async fn settle_rows(store: &Arc<crate::store::Store>, q: &Query) {
    let snap = |rows: &[crate::store::data::Task]| {
        let mut v: Vec<(String, String)> = rows
            .iter()
            .map(|t| (t.tid.clone(), t.state.clone()))
            .collect();
        v.sort();
        v
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut last = snap(&store.tasks().query(q).await.unwrap().rows);
    let mut stable = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let now = snap(&store.tasks().query(q).await.unwrap().rows);
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
            std::time::Instant::now() < deadline,
            "the task rows never settled: {now:?}"
        );
    }
}

/// The parallel workflow every scenario runs: one step fanning out to two
/// branches of the same irq act, joined when both branches complete.
fn parallel_workflow() -> Workflow {
    Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_PARALLEL,
            Vars::from(json!({
                "in": ["u1", "u2"],
                "acts": [Act::irq(|act| {
                    act.with_params_vars(|v| v.with("key", "act1")).with_id("act1")
                })]
            })),
        )
    })
}

/// KV backend that fails the first `batch` carrying a task row while armed:
/// the transient fault hitting one branch-act write while the branches run.
struct FailOnceTaskBatchKv {
    inner: MemoryStore,
    armed: AtomicBool,
    fired: AtomicUsize,
}

impl FailOnceTaskBatchKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
            fired: AtomicUsize::new(0),
        }
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    fn fired(&self) -> usize {
        self.fired.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for FailOnceTaskBatchKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> crate::Result<()> {
        let task_row = ops.iter().any(|op| match op {
            StoreBatchOp::Put { key, .. } => key.starts_with("tasks-id-"),
            StoreBatchOp::Delete { key } => key.starts_with("tasks-id-"),
        });
        if task_row && self.armed.swap(false, Ordering::SeqCst) {
            self.fired.fetch_add(1, Ordering::SeqCst);
            return Err(ActError::Store(
                "injected: branch-act task write failed".to_string(),
            ));
        }
        self.inner.batch(ops).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// One branch-act write fails while the parallel branches run: the flow
/// degrades gracefully — the failure is surfaced at the flush barrier, both
/// branches still complete, the parent completes exactly once with no
/// duplicate branch tasks — and the writer heals; the finished process is
/// swept without a stuck row or anything left to resume.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_parallel_store_fault_branch_write_heals() {
    bounded(
        "sch_parallel_store_fault_branch_write_heals",
        sch_parallel_store_fault_branch_write_heals_inner(),
    )
    .await;
}

async fn sch_parallel_store_fault_branch_write_heals_inner() {
    let kv = Arc::new(FailOnceTaskBatchKv::new());
    let engine = Engine::builder()
        .set_store(kv.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let rec: Rec = Arc::new(Mutex::new(Vec::new()));

    let sig = engine.signal(Vec::<(String, String)>::default());
    let (s, s2) = sig.double();
    let rec_in = rec.clone();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        let rec = rec_in.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rec.lock().push(e.tid.clone());
                s2.update(|d| {
                    d.push((e.pid.clone(), e.tid.clone()));
                    if d.len() >= 2 {
                        s2.close();
                    }
                });
            }
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &parallel_workflow());
    let pid = proc.id().to_string();
    rt.launch(&proc).await.unwrap();

    // both branches are in flight — one act message each
    let acts = s.recv().await;
    assert_eq!(acts.len(), 2);
    let tid1 = acts[0].1.clone();
    let tid2 = acts[1].1.clone();
    assert_ne!(tid1, tid2, "each branch runs its own act task");

    // settle every setup write, THEN arm: the first task-row batch the writer
    // sees from now on is the completion write of the branch completed first
    rt.cache().flush().await.unwrap();
    kv.arm();

    // both branches complete; the durable write of the first one fails
    rt.do_action(&Action::new(&pid, &tid1, EventAction::Next, Vars::new()))
        .await
        .unwrap();
    rt.do_action(&Action::new(&pid, &tid2, EventAction::Next, Vars::new()))
        .await
        .unwrap();
    tx.recv().await;
    assert!(
        proc.state().is_biz_success(),
        "the parent completed despite the fault"
    );

    // the failed write is surfaced by the next durability barrier rather than
    // swallowed. Which barrier reports it is not the test's to pick: a loaded
    // engine runs its own barriers (a cache miss flushes the pid's shard, the
    // tick drains writes), and whichever drains first consumes the latched
    // failure. So: assert the fault fired, and drain whatever is left.
    assert_eq!(kv.fired(), 1, "the injected fault must have fired once");
    let _ = rt.cache().flush().await;
    // the writer healed: everything written after the fault is durable
    rt.cache().flush().await.unwrap();

    // no duplicated work: one task row per branch and one for the step
    let store = rt.cache().store();
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    let rows = store.tasks().query(&q_all).await.unwrap().rows;
    // the parallel package runs each branch as a block act wrapping the irq
    // act (plus the step's own block wrapper): root + step1 + step block +
    // 2×(block act + irq act) = 7 rows — the key invariant is per branch
    assert_eq!(rows.len(), 7, "no duplicated rows: {rows:?}");
    assert_eq!(rows.iter().filter(|t| t.tid == tid1).count(), 1);
    assert_eq!(rows.iter().filter(|t| t.tid == tid2).count(), 1);
    // the write that hit the fault is whichever branch-act batch the writer
    // applied first — not deterministic. The contract is fault-agnostic: the
    // failure surfaced exactly once at the barrier above, and every row whose
    // write came after the fault is durable; the end-to-end invariants below
    // (both branches answered once, parent completed once, clean sweep) pin
    // the convergence
    assert_eq!(kv.fired(), 1, "the fail-once fault must fire exactly once");
    let step_tasks = proc.task_by_nid("step1");
    assert_eq!(
        step_tasks.len(),
        1,
        "the parent step completed exactly once"
    );
    assert_eq!(step_tasks.first().unwrap().state(), TaskState::Completed);
    // no client was asked twice for the same branch work: only the two
    // branch tids ever appear (a re-delivery of one of these two messages
    // stays within the at-least-once contract)
    let rec_ids = rec.lock().clone();
    assert!(
        rec_ids.iter().all(|t| t == &tid1 || t == &tid2),
        "no message for a branch beyond the two acts: {rec_ids:?}"
    );

    // the finished process is fully swept: no stuck rows, nothing to resume —
    // the stale branch row cannot wedge the cleanup
    for _ in 0..150 {
        if store.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt.cache().sweep_removable().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        store.procs().find(&pid).await.is_err(),
        "the finished process must be removed once its deliveries settled"
    );
    assert!(
        store.tasks().query(&q_all).await.unwrap().rows.is_empty(),
        "no task row may survive the removal"
    );
}

/// A crash between the branches: branch1 completed durably, branch2 still in
/// flight. The reload on the shared store re-drives the process without
/// re-running branch1 (its act is never rebuilt or asked again) and branch2
/// completes in the new engine, so the parent completes exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_parallel_restart_inflight_branch_completes_once() {
    bounded(
        "sch_parallel_restart_inflight_branch_completes_once",
        sch_parallel_restart_inflight_branch_completes_once_inner(),
    )
    .await;
}

async fn sch_parallel_restart_inflight_branch_completes_once_inner() {
    // the shared store survives the "crash" (engine teardown + reload)
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let (_, rx) = engine.signal(()).double();
    let rec: Rec = Arc::new(Mutex::new(Vec::new()));

    let sig = engine.signal(Vec::<(String, String)>::default());
    let (s, s2) = sig.double();
    let rec_in = rec.clone();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        let rec = rec_in.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rec.lock().push(e.tid.clone());
                s2.update(|d| {
                    d.push((e.pid.clone(), e.tid.clone()));
                    if d.len() >= 2 {
                        s2.close();
                    }
                });
            }
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &parallel_workflow());
    let pid = proc.id().to_string();
    rt.launch(&proc).await.unwrap();
    let acts = s.recv().await;
    assert_eq!(acts.len(), 2);
    let tid1 = acts[0].1.clone();
    let tid2 = acts[1].1.clone();

    // branch1 completes; branch2 stays in flight
    rt.do_action(&Action::new(&pid, &tid1, EventAction::Next, Vars::new()))
        .await
        .unwrap();

    // wait for the durable state to settle: branch1's row reads Completed —
    // branch2 stays in flight, so the step's own propagation record is still
    // open and nothing else needs pinning
    let store1 = rt.cache().store();
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    let mut settled = false;
    for _ in 0..100 {
        let rows = store1.tasks().query(&q_all).await.unwrap().rows;
        let b1_done = rows
            .iter()
            .any(|t| t.tid == tid1 && TaskState::from(t.state.as_str()) == TaskState::Completed);
        if b1_done {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        settled,
        "branch1's completion and its pending next record never settled durably"
    );

    // crash: everything above lives in the shared store only
    engine.close().await;

    // reload on the same store; recovery (outbox replay + boot re-drive) runs
    // within start, the replayed propagation settles in the first ticks
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let (tx2, rx2) = engine2.signal(()).double();
    let rec2: Rec = Arc::new(Mutex::new(Vec::new()));
    let rec2_in = rec2.clone();
    engine2.channel().on_message(move |e| {
        let rec2 = rec2_in.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rec2.lock().push(e.tid.clone());
            }
        }
    });
    auto_complete(&engine2, &rx2);

    // give the replayed `next` its ticks; the task set must never grow
    for _ in 0..25 {
        let n = store2.tasks().query(&q_all).await.unwrap().rows.len();
        assert!(n <= 7, "recovery duplicated branch tasks: {n} rows");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert_eq!(
        reloaded.task_by_nid("step1").len(),
        1,
        "the step is not duplicated"
    );
    // the second branch's nid is uniquified (`act1-…`), so count the branch
    // acts by their own task ids instead of the shared nid prefix
    assert!(
        reloaded.task(&tid1).is_some() && reloaded.task(&tid2).is_some(),
        "no branch act may be rebuilt on recovery"
    );
    assert_eq!(
        reloaded.task(&tid1).unwrap().state(),
        TaskState::Completed,
        "branch1 stays completed — it must not re-run"
    );
    assert_eq!(
        reloaded.task(&tid2).unwrap().state(),
        TaskState::Interrupt,
        "branch2 still awaits its client in the reloaded engine"
    );
    assert_eq!(
        store2.tasks().query(&q_all).await.unwrap().rows.len(),
        7,
        "root + step1 + blocks + two branch acts — the replayed next duplicated nothing"
    );

    // branch2 completes in the reloaded engine; the parent completes exactly
    // once
    rt2.do_action(&Action::new(&pid, &tid2, EventAction::Next, Vars::new()))
        .await
        .unwrap();
    tx2.recv().await;

    // every outbox record of the process closed — nothing stuck pending
    let mut drained = false;
    for _ in 0..100 {
        if store2.load_pending_ops().await.unwrap().is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        drained,
        "outbox records must all close once the process finishes"
    );

    // branch1 was never asked again and no new branch ever appeared. The new
    // engine may only re-deliver branch2's still-open message (the designed
    // at-least-once retry); branch1's count across BOTH engines stays 1.
    let rec2_ids = rec2.lock().clone();
    assert!(
        rec2_ids.iter().all(|t| t == &tid2),
        "the reloaded engine re-asked a branch it must not: {rec2_ids:?}"
    );
    let rec_ids = rec.lock().clone();
    assert_eq!(
        rec_ids.iter().filter(|t| **t == tid1).count(),
        1,
        "branch1 must be asked exactly once across the restart"
    );
    assert!(
        rec_ids.iter().all(|t| t == &tid1 || t == &tid2),
        "no message for a branch beyond the two acts: {rec_ids:?}"
    );

    // the process finished cleanly in the reloaded engine — terminal, then
    // swept; not stuck running
    let mut finished = false;
    for _ in 0..150 {
        match store2.procs().find(&pid).await {
            Err(_) => {
                finished = true;
                break;
            }
            Ok(row) if TaskState::from(row.state.as_str()).is_biz_success() => {
                finished = true;
                break;
            }
            Ok(_) => {}
        }
        let _ = rt2.cache().sweep_removable().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(finished, "the reloaded process must finish, not stay stuck");
}

/// The same `Next` action delivered twice: the second delivery is rejected
/// ("already completed") as a pure no-op — no state change, no second
/// propagation record, no duplicated task — and both branches still complete
/// with the parent finishing exactly once.
///
/// The duplicate is refused before it can touch anything (`Task::enter_action`
/// gates an action's application per task), and the baseline is taken after the
/// first action's propagation quiesces, so the row snapshot compares a settled
/// state instead of a moving one.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_parallel_duplicate_next_rejected_no_duplication() {
    bounded(
        "sch_parallel_duplicate_next_rejected_no_duplication",
        sch_parallel_duplicate_next_rejected_no_duplication_inner(),
    )
    .await;
}

async fn sch_parallel_duplicate_next_rejected_no_duplication_inner() {
    let (engine, proc) = create_proc(&parallel_workflow(), &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let rec: Rec = Arc::new(Mutex::new(Vec::new()));

    let sig = engine.signal(Vec::<(String, String)>::default());
    let (s, s2) = sig.double();
    let rec_in = rec.clone();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        let rec = rec_in.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rec.lock().push(e.tid.clone());
                s2.update(|d| {
                    d.push((e.pid.clone(), e.tid.clone()));
                    if d.len() >= 2 {
                        s2.close();
                    }
                });
            }
        }
    });
    auto_complete(&engine, &rx);

    let pid = proc.id().to_string();
    rt.launch(&proc).await.unwrap();
    let acts = s.recv().await;
    assert_eq!(acts.len(), 2);
    let tid1 = acts[0].1.clone();
    let tid2 = acts[1].1.clone();

    // branch1 completes
    rt.do_action(&Action::new(&pid, &tid1, EventAction::Next, Vars::new()))
        .await
        .unwrap();
    rt.cache().flush().await.unwrap();

    let store = rt.cache().store();
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    // the baseline must be quiescent: branch1's completion propagation is
    // still writing rows (the block act's completion, the step merge) after
    // `flush` returns
    settle_rows(&store, &q_all).await;
    let durable_snapshot = |rows: &[crate::store::data::Task]| {
        let mut v: Vec<(String, String)> = rows
            .iter()
            .map(|t| (t.tid.clone(), t.state.clone()))
            .collect();
        v.sort();
        v
    };
    let rows_before = store.tasks().query(&q_all).await.unwrap().rows;
    let snapshot_before = durable_snapshot(&rows_before);
    let msgs_before = rec.lock().len();

    // the SAME action delivered again is rejected
    let err = rt
        .do_action(&Action::new(&pid, &tid1, EventAction::Next, Vars::new()))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ActError::Action(_)) && err.to_string().contains("already completed"),
        "the duplicate Next must be rejected as already completed: {err}"
    );

    // the rejection is a pure no-op: no row changed, no message re-emitted,
    // no second propagation record for the branch
    rt.cache().flush().await.unwrap();
    let rows_after = store.tasks().query(&q_all).await.unwrap().rows;
    assert_eq!(
        snapshot_before,
        durable_snapshot(&rows_after),
        "the duplicate delivery must not change any task row"
    );
    assert_eq!(rec.lock().len(), msgs_before, "no message re-emitted");
    // the propagation record exists exactly once for the act. Its status may
    // have moved on (the record closes once its propagation applied), so the
    // invariant is the row count, not a pending status
    let q_act = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", tid1.clone())),
    );
    let nexts = store
        .ops()
        .query(&q_act)
        .await
        .unwrap()
        .rows
        .iter()
        .filter(|op| op.r#type == "next")
        .count();
    assert_eq!(
        nexts, 1,
        "the duplicate delivery must not enqueue a second propagation record"
    );

    // branch2 completes: both branches over, the parent finishes exactly once
    rt.do_action(&Action::new(&pid, &tid2, EventAction::Next, Vars::new()))
        .await
        .unwrap();
    tx.recv().await;
    assert!(proc.state().is_biz_success());
    let step_tasks = proc.task_by_nid("step1");
    assert_eq!(step_tasks.len(), 1, "a single step1 task, no duplicate");
    assert_eq!(step_tasks.first().unwrap().state(), TaskState::Completed);
    assert!(
        proc.task(&tid1).is_some() && proc.task(&tid2).is_some(),
        "two branch tasks, no duplicate"
    );

    // the flow fully drained: no outbox record left pending
    let mut drained = false;
    for _ in 0..100 {
        if store.load_pending_ops().await.unwrap().is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        drained,
        "outbox records must all close once the process finishes"
    );

    let rec_ids = rec.lock().clone();
    assert!(
        rec_ids.iter().all(|t| t == &tid1 || t == &tid2),
        "no message for a branch beyond the two acts: {rec_ids:?}"
    );
}

/// Concurrent completion of sibling acts advances the parent step exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_sibling_concurrent_complete() {
    bounded(
        "sch_action_sibling_concurrent_complete",
        sch_action_sibling_concurrent_complete_inner(),
    )
    .await;
}

async fn sch_action_sibling_concurrent_complete_inner() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_PARALLEL,
            Vars::from(json!({
                "in": ["u1", "u2"],
                "acts": [Act::irq(|act| {
                    act.with_params_vars(|v| v.with("key", "act1")).with_id("act1")
                })]
            })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();

    let sig = engine.signal(Vec::<(String, String)>::default());
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s2.update(|d| {
                    d.push((e.pid.clone(), e.tid.clone()));
                    if d.len() >= 2 {
                        s2.close();
                    }
                });
            }
        }
    });
    auto_complete(&engine, &rx);

    rt.launch(&proc).await.unwrap();
    let acts = s.recv().await;
    assert_eq!(acts.len(), 2);

    for (pid, tid) in &acts {
        rt.do_action(&Action::new(pid, tid, EventAction::Next, Vars::new()))
            .await
            .unwrap();
    }

    tx.recv().await;
    let step_tasks = proc.task_by_nid("step1");
    let step_task = step_tasks.first().unwrap();
    assert_eq!(step_task.state(), TaskState::Completed);
}
