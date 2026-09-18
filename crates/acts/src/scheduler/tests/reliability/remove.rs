//! Reliability contracts of the `Remove` client action: terminate one act in
//! place.
//!
//! The engine records every non-`Next` action as a durable outbox record
//! *before* applying it, so a store fault or a crash mid-action degrades
//! instead of corrupting: the write path surfaces the failure, the durable
//! state converges to exactly one terminal outcome, and recovery never
//! re-executes work twice. These cases pin that contract per condition:
//!
//! - a transient store failure under the action's state write is surfaced by
//!   the writer (`flush`) and heals without lost or duplicated work;
//! - a pending action record is re-applied exactly once after a restart;
//! - a duplicate delivery of the same action is rejected or a no-op;
//! - a finished process's message and delivery rows are swept with its others.

use std::sync::Arc;

use serial_test::serial;

use crate::{
    ActError, Action, ChannelOptions, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    store::{
        KvStore, MemoryStore,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::test::{USES_IRQ, auto_complete},
};

use super::*;

// ---------------------------------------------------------------------------
// REMOVE × 重启 — a pending Remove action record is re-applied exactly once
// after a restart.
// ---------------------------------------------------------------------------

/// A `Remove` whose outbox record landed but whose effects were never applied
/// (simulated crash between enqueue and apply) is re-applied by recovery on
/// reload: the act ends `Removed` exactly once, no duplicate act task is
/// created, and the process finishes so the sweeper removes every row — a
/// dropped replay would leave the act stuck `Interrupt` and its rows alive.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_recover_applies_pending_remove_op() {
    bounded(
        "sch_remove_recover_applies_pending_remove_op",
        sch_remove_recover_applies_pending_remove_op_inner(),
    )
    .await;
}

async fn sch_remove_recover_applies_pending_remove_op_inner() {
    // the shared store survives the "crash" (engine teardown + reload)
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

    let s = irq_created_signal(&engine, "act1");
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, tid) = s.recv().await;

    // crash before the Remove was applied: only the outbox record landed —
    // the act is still `Interrupt`, waiting for a client that never answers
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &tid, "remove", "{}")
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery must re-apply the Remove (the action path closes the
    // record itself)
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    poll_until(|| async {
        store2
            .load_pending_ops()
            .await
            .unwrap()
            .iter()
            .all(|op| op.r#type != "action")
    })
    .await;

    // exactly once, durably: the act row reads `Removed` — or, if engine2's
    // sweeper already ran the whole lifecycle to completion, nothing of the
    // process survives at all (also a pass: the replay was applied)
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    let settled_rows = poll_until(|| async {
        match store2.procs().find(&pid).await {
            // the sweeper deleted every row: the replayed remove ran to the end
            Err(_) => true,
            Ok(_) => store2
                .tasks()
                .query(&q)
                .await
                .unwrap()
                .rows
                .iter()
                .any(|t| t.tid == tid && t.state == "removed"),
        }
    })
    .await;
    if settled_rows {
        let procs_gone = store2.procs().find(&pid).await.is_err();
        if !procs_gone {
            // still mid-lifecycle: pin the durable shape — at most the one
            // re-applied act row (the sweeper may already be deleting rows)
            let task_rows = store2.tasks().query(&q).await.unwrap().rows;
            assert!(
                task_rows.iter().filter(|t| t.tid == tid).count() <= 1,
                "the re-applied remove must not create a second act task: {task_rows:?}"
            );
        }
    } else {
        panic!("the replayed remove never decided the act durably");
    }

    // the remove ended the whole workflow: after the sweeper runs, nothing of
    // the process survives
    sweep_until_gone(&rt2, &pid).await;

    engine2.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × DB失败 — the store fails the Remove's state write once; the
// degradation is surfaced and heals durably exactly once.
// ---------------------------------------------------------------------------

/// The durable write of the `Removed` state fails once (backend fault). The
/// action path degrades gracefully — the fault hits off the hot path and is
/// reported to the next durability barrier — and heals on its own: the
/// action's own `next` propagation re-persists the row, so the durable state
/// converges to `Removed` exactly once, the process finishes, and no work is
/// duplicated or lost.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_store_fail_degrades_and_heals_once() {
    bounded(
        "sch_remove_store_fail_degrades_and_heals_once",
        sch_remove_store_fail_degrades_and_heals_once_inner(),
    )
    .await;
}

async fn sch_remove_store_fail_degrades_and_heals_once_inner() {
    let kv = FailOnceTaskWriteKv::new();
    let store: Arc<dyn KvStore> = kv.clone();
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = two_step_irq_workflow();

    let s = irq_created_signal(&engine, "act1");
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // the transient fault lands exactly on the remove's durable state write
    kv.arm(&pid, &act1_tid);
    rt.do_action2(&pid, &act1_tid, EventAction::Remove, Vars::new())
        .await
        .unwrap();

    // graceful degradation, not silent corruption: the writer reports the
    // write it could not make durable
    let flush = rt.cache().flush().await;
    assert!(matches!(flush, Err(ActError::Store(_))), "{flush:?}");

    // the action did apply in memory and the workflow kept moving
    assert_eq!(
        proc.task(&act1_tid).unwrap().state(),
        TaskState::Removed,
        "the remove must decide the act's state even when its write failed"
    );

    // healing: the engine's own follow-up writes re-persist the row — the
    // durable state converges to `Removed` without any retry by the client
    let durable = rt.cache().store();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    poll_until(|| async {
        durable
            .tasks()
            .query(&q)
            .await
            .unwrap()
            .rows
            .iter()
            .any(|t| t.tid == act1_tid && t.state == "removed")
    })
    .await;

    // exactly once: the remove created no duplicate work — one act1 task and
    // one act2 task (scheduled by the remove's propagation, now waiting)
    assert_eq!(acts_of(&proc, "act1").len(), 1);
    let act2 = wait_waiting_act(&proc, "act2").await;
    assert_eq!(acts_of(&proc, "act2").len(), 1);

    rt.do_action2(&pid, &act2.id, EventAction::Next, Vars::new())
        .await
        .unwrap();
    poll_until(|| async { proc.state().is_completed() }).await;
    assert_settled(&proc);

    sweep_until_gone(&rt, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// REMOVE × 重复消息 — a duplicate Remove delivery is a terminal no-op.
// ---------------------------------------------------------------------------

/// Delivering `Remove` twice for the same act must not double-decide it: the
/// second delivery changes no state, touches no sibling, and schedules no
/// duplicate work — the act stays `Removed` exactly once and the process
/// still finishes through its second step.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_remove_duplicate_delivery_is_terminal_noop() {
    bounded(
        "sch_remove_duplicate_delivery_is_terminal_noop",
        sch_remove_duplicate_delivery_is_terminal_noop_inner(),
    )
    .await;
}

async fn sch_remove_duplicate_delivery_is_terminal_noop_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let workflow = two_step_irq_workflow();

    let s = irq_created_signal(&engine, "act1");
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    let action = Action::new(&pid, &act1_tid, EventAction::Remove, Vars::new());
    rt.do_action(&action).await.unwrap();

    // the remove completed step1 and scheduled step2; wait for act2 to wait
    let act2 = wait_waiting_act(&proc, "act2").await;

    // the duplicate delivery lands on the already-`Removed` act
    let second = rt.do_action(&action).await;
    println!("duplicate remove outcome: {}", outcome(&second));

    // no double decision, no collateral damage
    assert_eq!(
        proc.task(&act1_tid).unwrap().state(),
        TaskState::Removed,
        "the duplicate delivery must not change the decided state"
    );
    assert_eq!(
        act2.state(),
        TaskState::Interrupt,
        "the duplicate remove of act1 must not touch the sibling act2"
    );
    assert_eq!(
        acts_of(&proc, "act1").len(),
        1,
        "the duplicate delivery must not create a duplicate act1 task"
    );
    assert_eq!(
        acts_of(&proc, "act2").len(),
        1,
        "the duplicate delivery's propagation must not re-schedule step2"
    );

    // the process still finishes normally through its second step
    rt.do_action2(&pid, &act2.id, EventAction::Next, Vars::new())
        .await
        .unwrap();
    poll_until(|| async { proc.state().is_completed() }).await;
    assert_settled(&proc);

    // durable truth before cleanup: exactly one removed act row for the pid
    let store = rt.cache().store();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    let removed_rows = store
        .tasks()
        .query(&q)
        .await
        .unwrap()
        .rows
        .into_iter()
        .filter(|t| t.state == "removed")
        .count();
    assert_eq!(removed_rows, 1, "exactly one removed task row");

    sweep_until_gone(&rt, &pid).await;
    engine.close().await;
}

/// A finished process is always cleaned up — proc/task/outbox rows AND the
/// message/delivery rows its ack channel stored. Removal waits for the
/// process's event worker to drain first (the terminal message was already
/// delivered), so no in-flight emission can re-create rows afterwards.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_completed_proc_cleans_message_and_delivery_rows() {
    bounded(
        "sch_action_completed_proc_cleans_message_and_delivery_rows",
        sch_action_completed_proc_cleans_message_and_delivery_rows_inner(),
    )
    .await;
}

async fn sch_action_completed_proc_cleans_message_and_delivery_rows_inner() {
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
