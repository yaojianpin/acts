//! NEXT × store failure: a transient backend fault hitting the write path of
//! act1's `next` propagation must degrade gracefully — the flow continues from
//! the in-memory queue, the writer surfaces the failed write at the next
//! durability barrier, the next transition re-persists the lost row, and the
//! flow finishes exactly once with no lost, duplicated or stranded work.
//!
//! Two fault points are exercised, both at the exact moment act1's `Next` is
//! applied:
//!
//! - the step2 task row write (`WriteOp::Task`): the create write fails, the
//!   in-memory task stays, and s2's next state transition re-persists the row;
//! - the `next` outbox record create (`WriteOp::EnqueueNext`): the create
//!   fails, but the op pipeline's phase marks re-materialize the record, and
//!   recovery replays it idempotently without duplicating the in-flight s2.

use super::*;
use crate::{
    ActError, Action, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    scheduler::NodeKind,
    store::{
        KvStore, MemoryStore, ScanOptions, StoreBatchOp,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::test::{USES_IRQ, auto_complete},
};
use serial_test::serial;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Whether one batch writes the task row whose node is `nid` (top-level node
/// id — the root row embeds the whole workflow, so a substring match would
/// also hit it).
fn is_task_row_of(value: &[u8], nid: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(value)
        .ok()
        .and_then(|row| {
            row.get("node_data")?
                .as_str()
                .and_then(|node| serde_json::from_str::<serde_json::Value>(node).ok())
                .map(|node| node.get("id").and_then(|id| id.as_str()) == Some(nid))
        })
        .unwrap_or(false)
}

/// KV backend that fails the first armed `tasks-id-` batch carrying the step2
/// row and works normally afterwards: the transient store fault at the moment
/// act1's `next` schedules step2.
struct FailStep2TaskWriteKv {
    inner: MemoryStore,
    armed: AtomicBool,
    fired: AtomicUsize,
}

impl FailStep2TaskWriteKv {
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
impl KvStore for FailStep2TaskWriteKv {
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
        if self.armed.load(Ordering::SeqCst)
            && ops.iter().any(|op| match op {
                StoreBatchOp::Put { key, value } => {
                    key.starts_with("tasks-id-") && is_task_row_of(value, "s2")
                }
                _ => false,
            })
        {
            self.armed.store(false, Ordering::SeqCst);
            self.fired.fetch_add(1, Ordering::SeqCst);
            return Err(ActError::Store(
                "injected: step2 task write failed".to_string(),
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

/// KV backend that fails the first armed `ops-id-` batch and works normally
/// afterwards: the transient store fault that kills the durable `next`
/// outbox record act1's `next` enqueues.
struct FailNextRecordWriteKv {
    inner: MemoryStore,
    armed: AtomicBool,
    fired: AtomicUsize,
}

impl FailNextRecordWriteKv {
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
impl KvStore for FailNextRecordWriteKv {
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
        if self.armed.load(Ordering::SeqCst)
            && ops
                .iter()
                .any(|op| matches!(op, StoreBatchOp::Put { key, .. } if key.starts_with("ops-id-")))
        {
            self.armed.store(false, Ordering::SeqCst);
            self.fired.fetch_add(1, Ordering::SeqCst);
            return Err(ActError::Store(
                "injected: next outbox record write failed".to_string(),
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

/// The step2 task row write fails exactly when act1's `Next` schedules it:
/// the client action still succeeds, the flow continues in memory, the failed
/// write is surfaced at the next durability barrier, the next transition of
/// s2 re-persists the row, and the flow finishes exactly once with every task
/// present and nothing left behind.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_dbfail_step2_task_write_degrades_and_completes_exactly_once() {
    bounded(
        "sch_next_dbfail_step2_task_write_degrades_and_completes_exactly_once",
        sch_next_dbfail_step2_task_write_degrades_and_completes_exactly_once_inner(),
    )
    .await;
}

async fn sch_next_dbfail_step2_task_write_degrades_and_completes_exactly_once_inner() {
    let kv = Arc::new(FailStep2TaskWriteKv::new());
    let store: Arc<dyn KvStore> = kv.clone();
    let engine = Engine::builder().set_store(store).start().await.unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    let sig1 = engine.signal((String::new(), String::new()));
    let (s1, s1c) = sig1.double();
    engine.channel().on_message(move |e| {
        let s1c = s1c.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s1c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s1c.close();
            }
        }
    });
    let sig2 = engine.signal((String::new(), String::new()));
    let (s2, s2c) = sig2.double();
    engine.channel().on_message(move |e| {
        let s2c = s2c.clone();
        async move {
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s2c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2c.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s1.recv().await;

    // the fault: the first step2 row write after arming fails once — that is
    // the create write queued by act1's `next` propagation
    kv.arm();
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();

    // the propagation was not stopped by the store fault: step2 runs and its
    // act reaches the client
    let (_, act2_tid) = s2.recv().await;

    // the injected failure fired exactly once
    for _ in 0..250 {
        if kv.fired() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        kv.fired(),
        1,
        "the step2 write fault must have fired exactly once"
    );

    // the failure is not swallowed: the writer surfaces it to the next
    // durability barrier, and only once
    let err = rt.cache().flush().await.unwrap_err();
    assert!(
        matches!(err, ActError::Store(_)),
        "flush must surface the injected store failure, got: {err}"
    );
    rt.cache().flush().await.unwrap();

    // the healed write path re-persists step2 only at s2's NEXT state
    // transition (act2's completion) — while act2 waits for the client there
    // is no later write of s2's row, so nothing is asserted about the task
    // rows here; the end-to-end exactly-once assertions below cover the
    // durable convergence
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));

    // the act is consistent for the client: the already-applied next is
    // rejected, so a retrying client cannot double-complete act1
    assert!(
        rt.do_action(&Action::new(
            &pid,
            &act1_tid,
            EventAction::Next,
            Vars::new()
        ))
        .await
        .is_err(),
        "a duplicate next on act1 must be rejected"
    );

    // the flow finishes exactly once
    rt.do_action(&Action::new(
        &pid,
        &act2_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    tx.recv().await;
    assert!(proc.state().is_biz_success());
    assert_eq!(proc.task_by_nid("s1").len(), 1);
    assert_eq!(proc.task_by_nid("s2").len(), 1, "no duplicated step2");
    assert_eq!(proc.tasks().len(), 5, "no duplicated task anywhere");
    assert_eq!(kv.fired(), 1, "the fail-once fault must not re-fire");

    // nothing is left behind: the finished process sweeps cleanly
    let store1 = rt.cache().store();
    for _ in 0..150 {
        if store1.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store1.procs().find(&pid).await.is_err(),
        "the finished process must be removed once its deliveries settled"
    );
    assert!(
        store1.tasks().query(&q_all).await.unwrap().rows.is_empty(),
        "task rows must be gone with the process"
    );

    engine.close().await;
}

/// The `next` outbox record create fails when act1's `Next` is applied: the
/// op pipeline's own phase transitions re-materialize the record (each mark
/// is a full-row upsert), so after the transient fault the record is pending
/// again. A crash (engine close + reload over the same store) replays it
/// idempotently — `schedule_once` reuses the in-flight step2 instance instead
/// of duplicating it — and the resumed flow finishes exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_dbfail_next_record_lost_recovers_without_duplication() {
    bounded(
        "sch_next_dbfail_next_record_lost_recovers_without_duplication",
        sch_next_dbfail_next_record_lost_recovers_without_duplication_inner(),
    )
    .await;
}

async fn sch_next_dbfail_next_record_lost_recovers_without_duplication_inner() {
    let kv = Arc::new(FailNextRecordWriteKv::new());
    let store: Arc<dyn KvStore> = kv.clone();

    // first engine: act1's next record write fails, the flow runs on in memory
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();

    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    let sig1 = engine.signal((String::new(), String::new()));
    let (s1, s1c) = sig1.double();
    engine.channel().on_message(move |e| {
        let s1c = s1c.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s1c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s1c.close();
            }
        }
    });
    let sig2 = engine.signal((String::new(), String::new()));
    let (s2, s2c) = sig2.double();
    engine.channel().on_message(move |e| {
        let s2c = s2c.clone();
        async move {
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s2c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2c.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s1.recv().await;

    // the fault: the next outbox record create fails once
    kv.arm();
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();

    // the in-memory propagation was not stopped: step2 runs, act2 waits
    let (_, act2_tid) = s2.recv().await;
    for _ in 0..250 {
        if kv.fired() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        kv.fired(),
        1,
        "the outbox record write fault must have fired exactly once"
    );

    // drain the writer, then: the create failed, but the op pipeline's own
    // phase transitions re-materialize the record (each mark is a full-row
    // upsert). The record exists again — its status moves on with the flow
    // (the in-memory propagation completes and closes it), so only the
    // existence and the version bump pin the healing
    let _ = rt.cache().flush().await;
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    let mut healed = false;
    for _ in 0..100 {
        let rows = rt.cache().store().ops().query(&q_all).await.unwrap().rows;
        if rows
            .iter()
            .any(|op| op.tid == act1_tid && op.r#type == "next" && op.v >= 2)
        {
            healed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        healed,
        "the op pipeline must re-materialize the failed next record: {rows:?}",
        rows = rt.cache().store().ops().query(&q_all).await.unwrap().rows
    );

    // the tasks of the flow are all durable: the fault cost no task row
    let rows = rt.cache().store().tasks().query(&q_all).await.unwrap().rows;
    assert_eq!(rows.len(), 5, "root + s1 + s2 + act1 + act2 all durable");

    // the act is consistent for the client: a retry of act1's next is rejected
    assert!(
        rt.do_action(&Action::new(
            &pid,
            &act1_tid,
            EventAction::Next,
            Vars::new()
        ))
        .await
        .is_err(),
        "a duplicate next on act1 must be rejected"
    );

    // crash before act2 completes: act1's next record is gone with the fault
    engine.close().await;

    // reload from the same store: nothing to replay for act1 (no record), the
    // boot resume re-drives the in-flight s2/act2 at-least-once — without
    // duplicating anything
    let engine2 = Engine::builder().set_store(store).start().await.unwrap();
    let rt2 = engine2.runtime();
    let (tx2, rx2) = engine2.signal(()).double();
    auto_complete(&engine2, &rx2);
    let store2 = rt2.cache().store();

    for _ in 0..100 {
        if store2.tasks().query(&q_all).await.unwrap().rows.len() == 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        store2.tasks().query(&q_all).await.unwrap().rows.len(),
        5,
        "root + s1 + s2 + act1 + act2 — the lost record must not duplicate tasks"
    );

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert_eq!(reloaded.task_by_nid("s1").len(), 1);
    assert_eq!(reloaded.task_by_nid("s2").len(), 1);
    let acts = reloaded
        .tasks()
        .into_iter()
        .filter(|t| t.node().kind() == NodeKind::Act)
        .count();
    assert_eq!(acts, 2, "act1 + act2, no replayed duplicates");

    // finish the flow in the reloaded engine: exactly once, biz success
    rt2.do_action(&Action::new(
        &pid,
        &act2_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    tx2.recv().await;

    // the reloaded process is the one that finished — read its terminal state
    // back through engine2 (memory while resident, store once evicted). A
    // vanished process is a pass too: only a settled biz-success run is swept
    let mut biz_success = false;
    for _ in 0..150 {
        match rt2.proc(&pid).await {
            Ok(None) => {
                biz_success = true;
                break;
            }
            Ok(Some(p)) if p.state().is_biz_success() => {
                biz_success = true;
                break;
            }
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        biz_success,
        "the reloaded process must finish biz-success despite the lost record"
    );

    // nothing is left behind
    for _ in 0..150 {
        if store2.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt2.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store2.procs().find(&pid).await.is_err(),
        "the finished process must be removed once its deliveries settled"
    );
    assert!(
        store2.tasks().query(&q_all).await.unwrap().rows.is_empty(),
        "task rows must be gone with the process"
    );

    engine2.close().await;
}

/// A `Pending` outbox record (crash after enqueue, before `next` ran) is
/// re-dispatched on recovery.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_pending() {
    bounded(
        "sch_action_recover_pending",
        sch_action_recover_pending_inner(),
    )
    .await;
}

async fn sch_action_recover_pending_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));

    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2.close();
            }
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, tid) = s.recv().await;

    // Simulate a crash right after the action was applied and the `next`
    // outbox record was written, but before the queued `next` ran: persist the
    // task state and the `Pending` record (bypassing the in-memory queue).
    let task = proc.task(&tid).unwrap();
    task.set_state(TaskState::Completed);
    rt.cache().store().upsert_task(&task).await.unwrap();
    rt.cache()
        .store()
        .enqueue_next_op(&pid, &tid)
        .await
        .unwrap();

    // Recovery re-dispatches the pending outbox record idempotently.
    rt.recover_actions().await.unwrap();

    tx.recv().await;
    assert!(proc.state().is_biz_success());
}

/// A `next` that already completed is a no-op on recovery: the durable
/// The applied propagation phase stops re-propagation, so reloading after a crash
/// never duplicates tasks.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_completed_next_is_noop() {
    bounded(
        "sch_action_recover_completed_next_is_noop",
        sch_action_recover_completed_next_is_noop_inner(),
    )
    .await;
}

async fn sch_action_recover_completed_next_is_noop_inner() {
    // the shared store survives the "crash" (engine teardown + reload)
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());

    // first engine: run a two-step workflow to completion
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let (_, rx) = engine.signal(()).double();
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    let sig1 = engine.signal((String::new(), String::new()));
    let (s1, s1c) = sig1.double();
    engine.channel().on_message(move |e| {
        let s1c = s1c.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s1c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s1c.close();
            }
        }
    });
    let sig2 = engine.signal((String::new(), String::new()));
    let (s2, s2c) = sig2.double();
    engine.channel().on_message(move |e| {
        let s2c = s2c.clone();
        async move {
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s2c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2c.close();
            }
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s1.recv().await;
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();

    // act2 is now in flight and the process is still running; simulate a crash
    // that lost the outbox close for act1's already-run `next`
    let (_, act2_tid) = s2.recv().await;
    assert!(proc.state().is_running());
    rt.cache()
        .store()
        .enqueue_next_op(&pid, &act1_tid)
        .await
        .unwrap();
    engine.close().await;

    // reload from the same store: recovery re-dispatches the record, but the
    // durable applied propagation phase turns the re-run into a no-op
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();

    // wait for the recovery to settle: act2 stays in flight, so the process
    // keeps its legitimate pending outbox records — only the task set matters
    // (the replayed `next` must not duplicate s2/act2)
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    for _ in 0..100 {
        if store2.tasks().query(&q_all).await.unwrap().rows.len() == 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        store2.tasks().query(&q_all).await.unwrap().rows.len(),
        5,
        "root + s1 + s2 + act1 + act2 — the replayed next must not duplicate tasks"
    );

    // the outbox close is ordered after the persist: act1's stored vars row
    // must already carry the applied propagation phase (the async write was
    // drained by the flush barrier before the op was marked `Done`)
    let q = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", act1_tid.clone())),
    );
    let rows = store2.vars().query(&q).await.unwrap().rows;
    assert_eq!(rows.len(), 1);
    let data: Vars = serde_json::from_str(&rows[0].data).unwrap();
    let phase = data
        .get::<String>(crate::scheduler::PropagationPhase::task_key())
        .unwrap();
    assert_eq!(phase, "applied");

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert!(reloaded.state().is_running());

    // finish the flow in the reloaded engine: act2 completes, the process
    // finishes and every row of it is cleaned up
    rt2.do_action(&Action::new(
        &pid,
        &act2_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    // deletion is driven by the sweeper once the deliveries settled — drive
    // it directly instead of waiting for the timer tick
    for _ in 0..150 {
        if store2.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt2.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store2.procs().find(&pid).await.is_err(),
        "the finished process must be removed once its deliveries settled"
    );
    assert!(
        store2.tasks().query(&q_all).await.unwrap().rows.is_empty(),
        "task rows must be gone with the process"
    );
}

/// A crash mid-`next` (the next node was scheduled, propagation never finished)
/// is replayed without duplicating the already-scheduled task.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_partial_next_no_duplicate() {
    bounded(
        "sch_action_recover_partial_next_no_duplicate",
        sch_action_recover_partial_next_no_duplicate_inner(),
    )
    .await;
}

async fn sch_action_recover_partial_next_no_duplicate_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();

    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    // construct the durable state of a crash mid-`next`: s1 completed, its
    // `next` already created s2, but the propagation never finished
    let proc = rt.create_proc(&utils::longid(), &workflow);
    let pid = proc.id().to_string();
    proc.set_state(TaskState::Running);
    // scope the tree read guard so it is dropped before any await below
    let (root, s1, s2) = {
        let tree = proc.tree();
        let root = proc.create_task(tree.root.as_ref().unwrap(), None).unwrap();
        let s1 = proc
            .create_task(&tree.node("s1").unwrap(), Some(root.clone()))
            .unwrap();
        let s2 = proc
            .create_task(&tree.node("s2").unwrap(), Some(s1.clone()))
            .unwrap();
        (root, s1, s2)
    };
    root.set_state(TaskState::Running);
    s1.set_state(TaskState::Completed);
    s2.set_state(TaskState::Running);
    let store_ops = rt.cache().store();
    store_ops.upsert_proc(&proc).await.unwrap();
    store_ops.upsert_task(&root).await.unwrap();
    store_ops.upsert_task(&s1).await.unwrap();
    store_ops.upsert_task(&s2).await.unwrap();
    store_ops.enqueue_next_op(&pid, &s1.id).await.unwrap();
    engine.close().await;

    // reload: the boot resume re-drives the in-flight process (at-least-once),
    // so s2 — whose irq act child was never built before the crash — re-runs
    // and rebuilds exactly one act2; recovery re-dispatches s1's `next` on
    // top, and re-scheduling s2 is deduped. act2 stays in flight, so the
    // process keeps its legitimate pending outbox records; what matters is
    // that nothing is duplicated.
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    // wait for the re-run to settle: root + s1 + s2 + the rebuilt act2
    let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    for _ in 0..100 {
        if store2.tasks().query(&q_all).await.unwrap().rows.len() == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert_eq!(reloaded.task_by_nid("s1").len(), 1);
    assert_eq!(reloaded.task_by_nid("s2").len(), 1);
    let acts = reloaded
        .tasks()
        .into_iter()
        .filter(|t| t.node().kind() == crate::scheduler::NodeKind::Act)
        .collect::<Vec<_>>();
    assert_eq!(acts.len(), 1, "s2's act child rebuilt exactly once");
    assert_eq!(
        reloaded.tasks().len(),
        4,
        "root + s1 + s2 + act2, no duplicates"
    );
}

/// A `next` that stops with children still in flight (the parent step stays
/// `Running`) keeps its outbox record `Pending` — the propagation has not
/// finished — and the record is closed only after the children complete and
/// the step's `next` actually completes.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_next_op_pending_until_children_complete() {
    bounded(
        "sch_action_next_op_pending_until_children_complete",
        sch_action_next_op_pending_until_children_complete_inner(),
    )
    .await;
}

async fn sch_action_next_op_pending_until_children_complete_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2.close();
            }
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;
    let step_tid = proc.task_by_nid("s1").first().unwrap().id.clone();
    let store = rt.cache().store();

    // while act1 is still in flight (Interrupt), the step's `next` outbox
    // record must be `Pending`: the step cannot have completed, so the record
    // must not be closed early
    let mut found = false;
    for _ in 0..100 {
        if store
            .load_pending_ops()
            .await
            .unwrap()
            .iter()
            .any(|op| op.pid == pid && op.tid == step_tid)
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        found,
        "step's next outbox record should be Pending while the child act is in flight"
    );
    let q = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", step_tid.clone())),
    );
    let rows = store.ops().query(&q).await.unwrap().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "pending");

    // complete the act: the step then completes and closes its record
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    tx.recv().await;
    assert!(proc.state().is_biz_success());

    // both the act's and the step's records are eventually closed
    for _ in 0..100 {
        if store.load_pending_ops().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(store.load_pending_ops().await.unwrap().is_empty());
    let rows = store.ops().query(&q).await.unwrap().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].phase,
        crate::store::data::OpPhase::Completed.as_ref()
    );
}
