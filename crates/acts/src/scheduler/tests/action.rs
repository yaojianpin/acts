use serde_json::json;

use crate::{
    Act, Action, Config, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    scheduler::Sign,
    store::{KvStore, MemoryStore, query::{Expr, Filter, Query}},
    utils::{self, consts},
    utils::test::{USES_IRQ, USES_PARALLEL, auto_complete, create_proc},
};
use serial_test::serial;
use std::sync::Arc;

/// Completing the same act twice is rejected: the action application is
/// idempotent and the second call surfaces "already completed".
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_duplicate_complete() {
    let engine = Engine::new().start().unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));

    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
            s2.close();
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).unwrap();
    let (pid, tid) = s.recv().await;

    let action = Action::new(&pid, &tid, EventAction::Next, Vars::new());
    assert!(rt.do_action(&action).is_ok());
    assert!(rt.do_action(&action).is_err());

    tx.recv().await;
    assert!(proc.state().is_success());
}

/// A `Pending` outbox record (crash after enqueue, before `next` ran) is
/// re-dispatched on recovery.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_pending() {
    let engine = Engine::new().start().unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));

    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
            s2.close();
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).unwrap();
    let (pid, tid) = s.recv().await;

    // Simulate a crash right after the action was applied and the `next`
    // outbox record was written, but before the queued `next` ran: persist the
    // task state and the `Pending` record (bypassing the in-memory queue).
    let task = proc.task(&tid).unwrap();
    task.set_state(TaskState::Completed);
    rt.cache().store().upsert_task(&task).unwrap();
    rt.cache().store().enqueue_next_op(&pid, &tid).unwrap();

    // Recovery re-dispatches the pending outbox record idempotently.
    rt.recover_actions().unwrap();

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
    let mut config = Config::default();
    config.data.keep_processes = Some(true);

    // first engine: run a two-step workflow to completion
    let engine = Engine::new()
        .with_config(&config)
        .set_store(Some(store.clone()))
        .start()
        .unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
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
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            s1c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
            s1c.close();
        }
    });
    let sig2 = engine.signal((String::new(), String::new()));
    let (s2, s2c) = sig2.double();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            s2c.update(|d| *d = (e.pid.clone(), e.tid.clone()));
            s2c.close();
        }
    });
    auto_complete(&engine, &rx);

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).unwrap();
    let (pid, act1_tid) = s1.recv().await;
    rt.do_action(&Action::new(&pid, &act1_tid, EventAction::Next, Vars::new()))
        .unwrap();

    // s1's `next` schedules s2; complete act2 as well
    let (_, act2_tid) = s2.recv().await;
    rt.do_action(&Action::new(&pid, &act2_tid, EventAction::Next, Vars::new()))
        .unwrap();

    tx.recv().await;
    assert!(proc.state().is_success());

    // simulate a crash that lost the outbox close for act1's already-run `next`
    rt.cache().store().enqueue_next_op(&pid, &act1_tid).unwrap();
    engine.close();

    // reload from the same store: recovery re-dispatches the record, but the
    // durable NEXT_COMPLETE marker turns the re-run into a no-op
    let engine2 = Engine::new()
        .with_config(&config)
        .set_store(Some(store.clone()))
        .start()
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    for _ in 0..100 {
        if store2.load_pending_ops().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(store2.load_pending_ops().unwrap().is_empty());

    // the outbox close is ordered after the persist: act1's stored row must
    // already carry the NEXT_COMPLETE marker (the async write was drained by
    // the flush barrier before the op was marked `Done`)
    let q = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", act1_tid.clone())),
    );
    let rows = store2.tasks().query(&q).unwrap().rows;
    assert_eq!(rows.len(), 1);
    let data: Vars = serde_json::from_str(&rows[0].data).unwrap();
    let sign = data.get::<Sign>(consts::TASK_SIGN).unwrap();
    assert!(sign.contains(Sign::NEXT_COMPLETE));

    // the workflow was not re-propagated: same task set, no duplicates
    let reloaded = rt2.proc(&pid).unwrap().unwrap();
    assert!(reloaded.state().is_success());
    assert_eq!(reloaded.tasks().len(), 5, "root + s1 + s2 + act1 + act2");
}

/// A crash mid-`next` (the next node was scheduled, propagation never finished)
/// is replayed without duplicating the already-scheduled task.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_partial_next_no_duplicate() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::new().set_store(Some(store.clone())).start().unwrap();
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
    let tree = proc.tree();
    let root = proc.create_task(tree.root.as_ref().unwrap(), None);
    let s1 = proc.create_task(&tree.node("s1").unwrap(), Some(root.clone()));
    let s2 = proc.create_task(&tree.node("s2").unwrap(), Some(s1.clone()));
    root.set_state(TaskState::Running);
    s1.set_state(TaskState::Completed);
    s2.set_state(TaskState::Running);
    let store_ops = rt.cache().store();
    store_ops.upsert_proc(&proc).unwrap();
    store_ops.upsert_task(&root).unwrap();
    store_ops.upsert_task(&s1).unwrap();
    store_ops.upsert_task(&s2).unwrap();
    store_ops.enqueue_next_op(&pid, &s1.id).unwrap();
    engine.close();

    // reload: recovery re-dispatches s1's `next`; re-scheduling s2 is deduped
    let engine2 = Engine::new().set_store(Some(store.clone())).start().unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    for _ in 0..100 {
        if store2.load_pending_ops().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(store2.load_pending_ops().unwrap().is_empty());

    let reloaded = rt2.proc(&pid).unwrap().unwrap();
    assert_eq!(reloaded.task_by_nid("s1").len(), 1);
    assert_eq!(reloaded.task_by_nid("s2").len(), 1);
    assert_eq!(reloaded.tasks().len(), 3, "root + s1 + s2, no duplicates");
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

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();

    let sig = engine.signal(Vec::<(String, String)>::default());
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            s2.update(|d| {
                d.push((e.pid.clone(), e.tid.clone()));
                if d.len() >= 2 {
                    s2.close();
                }
            });
        }
    });
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    let acts = s.recv().await;
    assert_eq!(acts.len(), 2);

    for (pid, tid) in &acts {
        rt.do_action(&Action::new(pid, tid, EventAction::Next, Vars::new()))
            .unwrap();
    }

    tx.recv().await;
    let step_tasks = proc.task_by_nid("step1");
    let step_task = step_tasks.first().unwrap();
    assert_eq!(step_task.state(), TaskState::Completed);
}
