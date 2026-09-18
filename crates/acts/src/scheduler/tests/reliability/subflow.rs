//! Subflow reliability: the parent↔child handoff of `acts.core.subflow`
//! under a store fault, a restart, a duplicated delivery and concurrency.
//!
//! A subflow step starts a child process whose root carries the parent link
//! (`$parent_pid`/`$parent_tid`); when the child reaches a terminal state the
//! engine hands the outputs back by acting on the parent's subflow act
//! (`Runtime::return_to_act` → `do_action(Next, outputs)`). The four cases
//! here pin the reliability contract of that handoff:
//!
//! - a store fault on the child-start write must degrade the parent to a
//!   defined terminal state, leave no half-created child behind, and keep the
//!   engine healthy enough that a fresh run completes exactly once;
//! - a restart between the child start and its completion must recover both
//!   processes exactly once — completing the child on the new engine still
//!   delivers the back message and completes the parent, with no second child
//!   ever started and no stuck rows;
//! - a duplicated completion — the child's act completed twice, or the back
//!   message itself redelivered — must be rejected with no second side effect;
//! - two children racing their back messages must complete the parent exactly
//!   once, each child completing exactly once.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use super::*;
use serde_json::json;
use serial_test::serial;

use crate::{
    ActError, Engine, Executor, MessageState, Signal, Vars, Workflow,
    event::EventAction,
    scheduler::NodeKind,
    store::{
        KvStore, MemoryStore, ScanOptions, StoreBatchOp,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::test::{USES_IRQ, USES_PARALLEL, USES_SUBFLOW, create_proc},
};

/// The parent workflow: one subflow step calling `w2`.
fn main_flow() -> Workflow {
    Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
                "to": "w2",
            })),
        )
    })
}

/// The child workflow: one irq act an external client must complete.
fn child_flow() -> Workflow {
    Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    })
}

async fn deploy_w2(rt: &Arc<crate::scheduler::Runtime>, w2: &Workflow) {
    Executor::new(rt, &crate::Principal::unrestricted())
        .model()
        .deploy(w2, None)
        .await
        .unwrap();
}

/// Signal that closes when the `main` process reaches a terminal state:
/// `true` on completion, plain close on error. The subflow runs emit events
/// for BOTH processes, so a plain `auto_complete` (which closes on the first
/// complete event of any process) would release on the child instead.
fn main_done(engine: &Engine) -> (Signal<bool>, Signal<bool>) {
    let (tx, rx) = engine.signal(false).double();
    let channel = engine.channel();
    let rxc = rx.clone();
    channel.on_complete(move |e| {
        let rx = rxc.clone();
        async move {
            if e.mid == "main" {
                rx.update(|data| *data = true);
                rx.close();
            }
        }
    });
    let rxe = rx.clone();
    channel.on_error(move |e| {
        let rxe = rxe.clone();
        async move {
            if e.mid == "main" {
                rxe.close();
            }
        }
    });
    (tx, rx)
}

/// Count the `w2` child processes' completion events on the engine's channel.
fn count_child_completions(engine: &Engine) -> Arc<AtomicUsize> {
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    engine.channel().on_complete(move |e| {
        let c = c.clone();
        async move {
            if e.mid == "w2" {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    counter
}

/// Capture the (pid, act tid) of the child's `act1` irq message.
fn capture_child_act(engine: &Engine) -> Signal<(String, String)> {
    let sig = engine.signal((String::new(), String::new()));
    let (s, sc) = sig.double();
    engine.channel().on_message(move |e| {
        let sc = sc.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                sc.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                sc.close();
            }
        }
    });
    s
}

async fn count_procs(store: &crate::store::Store, mid: &str) -> usize {
    let q = Query::new().filter(Filter::and().expr(Expr::eq("mid", mid)));
    store.procs().query(&q).await.unwrap().rows.len()
}

/// Poll `ready` to hold within a deadline (150 × 20ms), the established
/// convention for waiting on async settling.
macro_rules! poll_until {
    ($ready:expr) => {{
        let mut ok = false;
        for _ in 0..150 {
            if $ready {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ok
    }};
}

/// Drive the sweeper until every row of both workflows is gone.
async fn sweep_all(rt: &Arc<crate::scheduler::Runtime>) {
    let store = rt.cache().store();
    let _ = poll_until!(
        count_procs(&store, "main").await == 0 && {
            let _ = rt.cache().sweep_removable().await;
            count_procs(&store, "w2").await == 0
        }
    );
    assert_eq!(
        count_procs(&store, "main").await,
        0,
        "no parent process row may survive the sweeper"
    );
    assert_eq!(
        count_procs(&store, "w2").await,
        0,
        "no child process row may survive the sweeper"
    );
}

/// KV backend that fails exactly ONE batch carrying a proc row — the second
/// one: the first is the parent's launch, the second is the subflow's
/// `start_as_owner` writing the child. The fault models a transient backend
/// outage at the exact moment a subflow spawns its child.
struct FailOnceSubflowStartKv {
    inner: MemoryStore,
    armed: AtomicBool,
    spent: AtomicBool,
    faults: AtomicUsize,
}

impl FailOnceSubflowStartKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
            spent: AtomicBool::new(false),
            faults: AtomicUsize::new(0),
        }
    }

    fn faults(&self) -> usize {
        self.faults.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for FailOnceSubflowStartKv {
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
        let starts_proc = ops
            .iter()
            .any(|op| matches!(op, StoreBatchOp::Put { key, .. } if key.starts_with("procs-id-")));
        if starts_proc && !self.spent.load(Ordering::SeqCst) {
            if self.armed.swap(false, Ordering::SeqCst) {
                self.spent.store(true, Ordering::SeqCst);
                self.faults.fetch_add(1, Ordering::SeqCst);
                return Err(ActError::Store("injected: backend unavailable".to_string()));
            }
            self.armed.store(true, Ordering::SeqCst);
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

/// DB失败: the store fault hits the child-start write. The parent must
/// degrade to a defined terminal state (error) instead of hanging, the failed
/// start must leave no child row behind (no orphans), and the engine — with
/// the store healed — must complete a fresh parent exactly once with exactly
/// one child process.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_storefault_child_start_degrades_and_heals() {
    bounded(
        "sch_subflow_storefault_child_start_degrades_and_heals",
        sch_subflow_storefault_child_start_degrades_and_heals_inner(),
    )
    .await;
}

async fn sch_subflow_storefault_child_start_degrades_and_heals_inner() {
    let main = main_flow();
    let w2 = child_flow();

    let kv = Arc::new(FailOnceSubflowStartKv::new());
    let engine = Engine::builder()
        .set_store(kv.clone() as Arc<dyn KvStore>)
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();

    deploy_w2(&rt, &w2).await;

    // run 1: the injected fault hits the child-start write
    let (tx, _) = main_done(&engine);
    let proc = rt.create_proc(&utils::longid(), &main);
    rt.launch(&proc).await.unwrap();
    let completed = tx.recv().await;
    assert!(!completed, "the faulted run must not complete");

    // graceful degradation: the parent ends in a defined terminal state, not
    // stuck mid-flight, with the store fault as its cause
    assert!(
        proc.state().is_error(),
        "the parent must degrade to error, got {}",
        proc.state()
    );
    assert_eq!(
        kv.faults(),
        1,
        "the child-start write must have failed exactly once"
    );

    // no orphans: the failed start left no child row — the parent is the only
    // process ever written
    let store = rt.cache().store();
    assert!(
        poll_until!(count_procs(&store, "w2").await == 0),
        "a failed child start must not write a child process row"
    );
    assert_eq!(
        count_procs(&store, "main").await,
        1,
        "only the faulted parent's row exists"
    );

    // heal: the wrapper is spent, the store serves writes again — a fresh
    // parent completes exactly once with exactly one child process
    let (tx2, _) = main_done(&engine);
    let w2_done = count_child_completions(&engine);
    let child_sig = capture_child_act(&engine);

    let proc2 = rt.create_proc(&utils::longid(), &main);
    let parent2_pid = proc2.id().to_string();
    rt.launch(&proc2).await.unwrap();
    let (child_pid, act1_tid) = child_sig.recv().await;
    assert_ne!(
        parent2_pid, child_pid,
        "the child is a process of its own, not the parent"
    );
    assert!(
        poll_until!(count_procs(&store, "w2").await == 1),
        "exactly one child process row must exist across both runs, got {}",
        count_procs(&store, "w2").await
    );

    rt.do_action2(&child_pid, &act1_tid, EventAction::Next, Vars::new())
        .await
        .unwrap();
    assert!(
        tx2.recv().await,
        "the healed run's parent must complete exactly once"
    );
    assert!(proc2.state().is_biz_success());
    assert!(
        poll_until!(w2_done.load(Ordering::SeqCst) == 1),
        "the child must complete exactly once, got {}",
        w2_done.load(Ordering::SeqCst)
    );

    // no orphans: every row of both runs is eventually cleaned up
    sweep_all(&rt).await;
    engine.close().await;
}

/// 重启: the child is started, the parent's subflow act is in flight, and the
/// engine "crashes" (close + reload on the shared store). Completing the
/// child on the new engine must deliver the back message across the restart:
/// the child completes exactly once, the parent is not stuck, no second child
/// process row ever existed, and recovery left both processes unduplicated.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_restart_child_back_message_completes_parent() {
    bounded(
        "sch_subflow_restart_child_back_message_completes_parent",
        sch_subflow_restart_child_back_message_completes_parent_inner(),
    )
    .await;
}

async fn sch_subflow_restart_child_back_message_completes_parent_inner() {
    let main = main_flow();
    let w2 = child_flow();

    // the shared store survives the crash
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();

    deploy_w2(&rt, &w2).await;
    let child_sig = capture_child_act(&engine);

    let proc = rt.create_proc(&utils::longid(), &main);
    let parent_pid = proc.id().to_string();
    rt.launch(&proc).await.unwrap();
    let (child_pid, act1_tid) = child_sig.recv().await;

    // the parent is mid-subflow: exactly one subflow act task, in flight
    let acts: Vec<_> = proc
        .tasks()
        .into_iter()
        .filter(|t| t.node().kind() == NodeKind::Act)
        .collect();
    assert_eq!(acts.len(), 1, "exactly one subflow act task");
    assert!(
        !proc.state().is_completed(),
        "the parent is still in flight"
    );

    // exactly one child process row ever existed before the crash
    let store1 = rt.cache().store();
    assert!(
        poll_until!(count_procs(&store1, "w2").await == 1),
        "exactly one child process row before the restart"
    );
    engine.close().await;

    // reload from the same store
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let (tx2, _) = main_done(&engine2);
    let w2_done = count_child_completions(&engine2);

    // recovery restored both processes exactly as they were: nothing
    // duplicated, nothing re-run — the parent act waits, the child act waits
    let parent2 = rt2
        .proc(&parent_pid)
        .await
        .unwrap()
        .expect("the parent must be restored after the restart");
    let child2 = rt2
        .proc(&child_pid)
        .await
        .unwrap()
        .expect("the child must be restored after the restart");
    assert_eq!(
        parent2.tasks().len(),
        proc.tasks().len(),
        "recovery must not duplicate parent tasks"
    );
    assert_eq!(
        child2
            .tasks()
            .into_iter()
            .filter(|t| t.node().kind() == NodeKind::Act)
            .count(),
        1,
        "the child keeps exactly one act task"
    );
    assert!(
        child2.task(&act1_tid).unwrap().state().is_interrupted(),
        "the child act is still waiting for its client action"
    );
    // the boot replay must not have started a second child
    let store2 = rt2.cache().store();
    assert!(
        poll_until!(count_procs(&store2, "w2").await == 1),
        "recovery must not start a second child process"
    );

    // complete the child on the new engine: the back message crosses the
    // restart and completes the parent
    rt2.do_action2(&child_pid, &act1_tid, EventAction::Next, Vars::new())
        .await
        .unwrap();
    assert!(
        tx2.recv().await,
        "the parent must complete after the child's back message"
    );
    assert!(
        child2.state().is_completed(),
        "the child completed exactly once"
    );
    assert!(parent2.state().is_biz_success(), "the parent is not stuck");
    assert!(
        poll_until!(w2_done.load(Ordering::SeqCst) == 1),
        "exactly one child completion event, got {}",
        w2_done.load(Ordering::SeqCst)
    );

    // no stuck rows: everything is cleaned up exactly once
    sweep_all(&rt2).await;
    engine2.close().await;
}

/// 重复消息: the child's completion is delivered twice — the same action
/// re-issued on the child act, and the back message itself redelivered to the
/// parent act. The second delivery of each must be rejected, and the parent
/// completes exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_duplicate_back_message_is_noop() {
    bounded(
        "sch_subflow_duplicate_back_message_is_noop",
        sch_subflow_duplicate_back_message_is_noop_inner(),
    )
    .await;
}

async fn sch_subflow_duplicate_back_message_is_noop_inner() {
    let main = main_flow();
    let w2 = child_flow();

    let (engine, proc) = create_proc(&main, &utils::longid()).await;
    let rt = engine.runtime();

    deploy_w2(&rt, &w2).await;
    let (tx, _) = main_done(&engine);
    let main_done_count = {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        engine.channel().on_complete(move |e| {
            let c = c.clone();
            async move {
                if e.mid == "main" {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        counter
    };
    let w2_done = count_child_completions(&engine);
    let child_sig = capture_child_act(&engine);

    let parent_pid = proc.id().to_string();
    rt.launch(&proc).await.unwrap();
    let (child_pid, act1_tid) = child_sig.recv().await;
    let parent_act_tid = proc
        .tasks()
        .into_iter()
        .find(|t| t.node().kind() == NodeKind::Act)
        .expect("the parent has one subflow act task")
        .id
        .clone();

    // first delivery: the child completes and its back message drives the
    // parent act
    rt.do_action2(&child_pid, &act1_tid, EventAction::Next, Vars::new())
        .await
        .unwrap();
    assert!(
        tx.recv().await,
        "the parent completes on the first delivery"
    );

    // duplicate #1: the same child completion re-issued is rejected
    let second = rt
        .do_action2(&child_pid, &act1_tid, EventAction::Next, Vars::new())
        .await;
    assert!(
        second.is_err(),
        "the duplicate child completion must be rejected: {second:?}"
    );

    // duplicate #2: the back message itself redelivered to the (now
    // completed) parent act is rejected
    let redelivered = rt
        .do_action2(&parent_pid, &parent_act_tid, EventAction::Next, Vars::new())
        .await;
    assert!(
        redelivered.is_err(),
        "the redelivered back message must be rejected: {redelivered:?}"
    );

    // exactly-once: one child completion, one parent completion, and the
    // process state is stable
    assert!(
        poll_until!(w2_done.load(Ordering::SeqCst) == 1),
        "the child completed exactly once, got {}",
        w2_done.load(Ordering::SeqCst)
    );
    assert_eq!(
        main_done_count.load(Ordering::SeqCst),
        1,
        "the parent completed exactly once"
    );
    assert!(proc.state().is_biz_success());
    engine.close().await;
}

/// 并发: a parallel step whose two branches are subflows starts two children
/// in flight at once; completing both children concurrently races their back
/// messages. The parent completes exactly once and each child completes
/// exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_concurrent_children_complete_parent_once() {
    bounded(
        "sch_subflow_concurrent_children_complete_parent_once",
        sch_subflow_concurrent_children_complete_parent_once_inner(),
    )
    .await;
}

async fn sch_subflow_concurrent_children_complete_parent_once_inner() {
    // two branches, both calling the same child workflow: the parallel step
    // wraps each `in` item's acts in a parallel block, so both children are
    // started at once
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_PARALLEL,
            Vars::from(json!({
                "in": ["u1", "u2"],
                "acts": [json!({
                    "uses": USES_SUBFLOW,
                    "params": {"to": "w2"},
                })],
            })),
        )
    });
    let w2 = child_flow();

    let (engine, proc) = create_proc(&main, &utils::longid()).await;
    let rt = engine.runtime();

    deploy_w2(&rt, &w2).await;
    let (tx, _) = main_done(&engine);
    let w2_done = count_child_completions(&engine);

    // collect both children's (pid, act tid) — closed when two arrived
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

    rt.launch(&proc).await.unwrap();
    let children = s.recv().await;
    assert_eq!(children.len(), 2, "two subflow branches, two children");
    let pids: std::collections::HashSet<_> = children.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(pids.len(), 2, "the branches are distinct child processes");

    // both children are in flight at the same time
    let store = rt.cache().store();
    assert!(
        poll_until!(count_procs(&store, "w2").await == 2),
        "both children must be in flight simultaneously"
    );

    // complete both children concurrently — their back messages race
    let mut handles = Vec::with_capacity(2);
    for (pid, tid) in children {
        let rt = rt.clone();
        handles.push(tokio::spawn(async move {
            rt.do_action2(&pid, &tid, EventAction::Next, Vars::new())
                .await
        }));
    }
    for handle in handles {
        handle
            .await
            .expect("the concurrent completion task must not panic")
            .expect("each child completes exactly once");
    }

    assert!(
        tx.recv().await,
        "the parent completes exactly once after both back messages"
    );
    assert!(proc.state().is_biz_success());
    assert!(
        poll_until!(w2_done.load(Ordering::SeqCst) == 2),
        "each child completes exactly once, got {}",
        w2_done.load(Ordering::SeqCst)
    );

    // no orphans: both children and the parent are cleaned up
    sweep_all(&rt).await;
    engine.close().await;
}
