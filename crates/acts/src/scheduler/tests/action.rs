use serde_json::json;

use crate::{
    Act, Action, ChannelOptions, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    scheduler::Sign,
    store::{
        KvStore, MemoryStore,
        query::{Expr, Filter, Query},
    },
    utils::test::{USES_IRQ, USES_PARALLEL, auto_complete, create_proc},
    utils::{self, consts},
};
use serial_test::serial;
use std::sync::Arc;

/// Completing the same act twice is rejected: the action application is
/// idempotent and the second call surfaces "already completed".
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_duplicate_complete() {
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

    let action = Action::new(&pid, &tid, EventAction::Next, Vars::new());
    assert!(rt.do_action(&action).await.is_ok());
    assert!(rt.do_action(&action).await.is_err());

    tx.recv().await;
    assert!(proc.state().is_success());
}

/// A `Pending` outbox record (crash after enqueue, before `next` ran) is
/// re-dispatched on recovery.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_pending() {
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
    assert!(proc.state().is_success());
}

/// A `next` that already completed is a no-op on recovery: the durable
/// `NEXT_COMPLETE` marker stops re-propagation, so reloading after a crash
/// never duplicates tasks.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_completed_next_is_noop() {
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
    // durable NEXT_COMPLETE marker turns the re-run into a no-op
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
    // must already carry the NEXT_COMPLETE marker (the async write was drained
    // by the flush barrier before the op was marked `Done`)
    let q = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", act1_tid.clone())),
    );
    let rows = store2.vars().query(&q).await.unwrap().rows;
    assert_eq!(rows.len(), 1);
    let data: Vars = serde_json::from_str(&rows[0].data).unwrap();
    let sign = data.get::<Sign>(consts::TASK_SIGN).unwrap();
    assert!(sign.contains(Sign::NEXT_COMPLETE));

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
    assert!(proc.state().is_success());

    // both the act's and the step's records are eventually closed
    for _ in 0..100 {
        if store.load_pending_ops().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(store.load_pending_ops().await.unwrap().is_empty());
}

/// A non-`Next` action whose outbox record landed but whose task state write
/// was lost in the crash is re-applied on recovery: the client's action is not
/// silently dropped.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_lost_action() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    // two steps: skipping act1 must not finish the process — the re-applied
    // skip advances to act2, which stays in flight, so the process rows
    // survive for inspection
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
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

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // simulate a crash mid-action: the Skip was applied in memory and its
    // outbox record landed, but the task state write never became durable —
    // the store still holds the pre-action (Interrupt) row
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act1_tid, "skip", "{}")
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery re-applies the Skip action, which closes the record
    // (the process keeps its legitimate pending `next` records while act2 is
    // in flight — only the action record must drain)
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "skip action record was not closed");

    // the process is still running (act2 in flight): its rows survive
    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    let act_task = reloaded.task(&act1_tid).unwrap();
    assert_eq!(act_task.state(), TaskState::Skipped);
    assert_eq!(
        reloaded.task_by_nid("s2").len(),
        1,
        "the re-applied skip must advance to s2"
    );
    assert!(reloaded.state().is_running());
}

/// A `Cancel` action whose outbox record landed but whose effects were never
/// durably applied is re-applied on recovery — even though the target act is
/// already `Completed` (from the earlier `Next`), which would otherwise make
/// the terminal-state check close the record and silently drop the cancel.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_cancel() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
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
    let sig2 = engine.signal((String::new(), String::new()));
    let (s3, s4) = sig2.double();
    engine.channel().on_message(move |e| {
        let s4 = s4.clone();
        async move {
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s4.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s4.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // complete act1 so step1 completes and act2 is scheduled
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    let (_, act2_tid) = s3.recv().await;

    // simulate a crash before the cancel was applied: only the outbox record
    // landed — act2 is still in flight (Interrupt), nothing was cancelled
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act1_tid, "cancel", r#"{"to":"step1"}"#)
        .await
        .unwrap();
    engine.close().await;

    // reload: the cancel must be re-applied — act2 becomes Cancelled even
    // though act1 (the cancel target) is already Completed
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    // the cancel action record must be closed; the still-running root and the
    // redo act keep their `next` records open by design
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "cancel action record was not closed");

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    let act2_task = reloaded.task(&act2_tid).unwrap();
    assert_eq!(act2_task.state(), TaskState::Cancelled);
}

/// An `Abort` action whose outbox record landed but whose task write was lost
/// is re-applied on recovery: the act and its ancestors become `Aborted`.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_abort() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
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

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // crash before the abort was durably applied: only the outbox record landed
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act1_tid, "abort", "{}")
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery re-applies the abort to the act and its ancestors
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "abort action record was not closed");

    // the abort terminates the process; finished processes are cleaned up by
    // the sweeper once their deliveries settled — if the recovery had
    // silently dropped the abort, act1 would still be Interrupt and the
    // process rows would survive
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    for _ in 0..150 {
        if store2.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt2.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store2.procs().find(&pid).await.is_err(),
        "the aborted process must be removed after the re-applied abort"
    );
    assert!(
        store2.tasks().query(&q).await.unwrap().rows.is_empty(),
        "the aborted process's task rows must be gone"
    );
}

/// An `Error` action whose outbox record landed but whose task write was lost
/// is re-applied on recovery: the act becomes `Error` with the recorded code.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_error() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
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

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // crash before the error was durably applied: only the outbox record landed
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act1_tid, "error", r#"{"ecode":"err1"}"#)
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery re-applies the error with its code
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "error action record was not closed");

    // the error terminates the process; finished processes are cleaned up by
    // the sweeper once their deliveries settled — if the recovery had
    // silently dropped the error, act1 would still be Interrupt and the
    // process rows would survive
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    for _ in 0..150 {
        if store2.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt2.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store2.procs().find(&pid).await.is_err(),
        "the errored process must be removed after the re-applied error"
    );
    assert!(
        store2.tasks().query(&q).await.unwrap().rows.is_empty(),
        "the errored process's task rows must be gone"
    );
}

/// A `Back` action whose outbox record landed but whose task write was lost is
/// re-applied on recovery: the act becomes `Backed` and the redo task resumes.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_back() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
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
    let sig2 = engine.signal((String::new(), String::new()));
    let (s3, s4) = sig2.double();
    engine.channel().on_message(move |e| {
        let s4 = s4.clone();
        async move {
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s4.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s4.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // complete act1 so step1 completes and act2 is scheduled
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    let (_, act2_tid) = s3.recv().await;

    // crash before the back was durably applied: only the outbox record landed
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act2_tid, "back", r#"{"to":"step1"}"#)
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery re-applies the back
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "back action record was not closed");

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert_eq!(reloaded.task(&act2_tid).unwrap().state(), TaskState::Backed);
}

/// A `Back` whose effects are durable but whose outbox close was lost is
/// closed on recovery without re-applying — the redo task is not duplicated.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_closes_applied_back() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
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
    let sig2 = engine.signal((String::new(), String::new()));
    let (s3, s4) = sig2.double();
    engine.channel().on_message(move |e| {
        let s4 = s4.clone();
        async move {
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s4.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s4.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    let (_, act2_tid) = s3.recv().await;

    // apply the back normally (durable: act2 Backed + redo task created), then
    // simulate a crash that lost only the outbox close
    let mut options = Vars::new();
    options.set("to", "step1");
    rt.do_action(&Action::new(&pid, &act2_tid, EventAction::Back, options))
        .await
        .unwrap();
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act2_tid, "back", r#"{"to":"step1"}"#)
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery sees act2 already Backed (terminal) and closes the
    // record without re-applying — no second redo task
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "back action record was not closed");

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert_eq!(reloaded.task(&act2_tid).unwrap().state(), TaskState::Backed);
    assert_eq!(
        reloaded.task_by_nid("step1").len(),
        2,
        "original + one redo"
    );
}

/// A non-`Next` action whose state write is durable but whose outbox close was
/// lost is closed on recovery without re-applying (no duplicate effects), and
/// the task's messages are marked completed so the client is not asked to act
/// again.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_closes_applied_action() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let (_, rx) = engine.signal(()).double();
    // two steps: the skipped act1 advances to act2, which stays in flight, so
    // the process keeps running and its rows survive for inspection
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    // ack-enabled channel: messages are persisted to the store
    let chan = engine.channel_with_options(&ChannelOptions {
        id: "chan1".to_string(),
        ack: true,
        ..Default::default()
    });
    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    let sig3 = engine.signal((String::new(), String::new()));
    let (s3, s3c) = sig3.double();
    chan.on_message(move |e| {
        let s2 = s2.clone();
        let s3c = s3c.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2.close();
            } else if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                s3c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s3c.close();
            }
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // apply the Skip action normally (durable), then simulate a crash that
    // lost only the outbox close
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Skip,
        Vars::new(),
    ))
    .await
    .unwrap();
    // the skip advanced the workflow: act2 is in flight, the process runs on
    s3.recv().await;
    assert!(proc.state().is_running());
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act1_tid, "skip", "{}")
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery sees the task already Skipped (terminal) and closes the
    // record without re-applying (the process keeps its legitimate pending
    // `next` records while act2 is in flight — only the action record drains);
    // the act message is marked completed
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "skip action record was not closed");

    let reloaded = rt2.proc(&pid).await.unwrap().unwrap();
    assert_eq!(reloaded.task_by_nid("s1").len(), 1);
    let act_task = reloaded.task(&act1_tid).unwrap();
    assert_eq!(act_task.state(), TaskState::Skipped);
    assert_eq!(
        reloaded.task_by_nid("s2").len(),
        1,
        "the process stays running at act2 — nothing was re-applied"
    );
    assert!(reloaded.state().is_running());

    // the act deliveries are completed: the client will not be asked again
    let q = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", act1_tid.clone())),
    );
    let deliveries = store2.deliveries().query(&q).await.unwrap().rows;
    assert!(!deliveries.is_empty());
    assert!(
        deliveries
            .iter()
            .all(|d| d.status == crate::data::DeliveryStatus::Completed)
    );
}

/// Concurrent completion of sibling acts advances the parent step exactly once.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_sibling_concurrent_complete() {
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

/// A finished process is always cleaned up — proc/task/outbox rows AND the
/// message/delivery rows its ack channel stored. Removal waits for the
/// process's event worker to drain first (the terminal message was already
/// delivered), so no in-flight emission can re-create rows afterwards.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_completed_proc_cleans_message_and_delivery_rows() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    // ack-enabled channel: every delivered event is stored as canonical
    // message + delivery rows of the process
    let chan = engine.channel_with_options(&ChannelOptions {
        id: "chan-clean".to_string(),
        ack: true,
        ..Default::default()
    });
    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    chan.on_message(move |e| {
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
    let pid = proc.id().to_string();
    rt.launch(&proc).await.unwrap();
    let (_, act1_tid) = s.recv().await;

    // the running act's message was stored (delivery rows exist)
    let store = rt.cache().store();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    assert!(
        !store.messages().query(&q).await.unwrap().rows.is_empty(),
        "the emitted message must be stored while the process runs"
    );

    // complete the act: the process finishes and is cleaned up
    rt.do_action(&Action::new(
        &pid,
        &act1_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    tx.recv().await;

    // deletion is driven by the sweeper once the deliveries settled — drive
    // it directly instead of waiting for the timer tick
    for _ in 0..150 {
        if store.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store.procs().find(&pid).await.is_err(),
        "the finished process must be removed once its deliveries settled"
    );
    assert!(
        store.tasks().query(&q).await.unwrap().rows.is_empty(),
        "task rows must be gone with the process"
    );
    assert!(
        store.ops().query(&q).await.unwrap().rows.is_empty(),
        "outbox rows must be gone with the process"
    );
    assert!(
        store.messages().query(&q).await.unwrap().rows.is_empty(),
        "message rows must be gone with the process"
    );
    assert!(
        store.deliveries().query(&q).await.unwrap().rows.is_empty(),
        "delivery rows must be gone with the process"
    );

    // no in-flight worker emission re-created rows after the removal
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        store.messages().query(&q).await.unwrap().rows.is_empty(),
        "no message row may reappear after the removal"
    );
    assert!(
        store.deliveries().query(&q).await.unwrap().rows.is_empty(),
        "no delivery row may reappear after the removal"
    );
}
