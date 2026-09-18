//! Reliability contracts of the `Cancel` client action: rewind a completed
//! step and redo it.
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
//! - a `Cancel` racing a `Complete` on the same act tid converges: the act
//!   ends in exactly one terminal state and the process always settles.

use std::sync::Arc;
use std::time::Instant;

use serial_test::serial;

use crate::{
    ActError, Action, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    store::{KvStore, MemoryStore},
    utils,
    utils::test::USES_IRQ,
};

use super::*;

// ---------------------------------------------------------------------------
// CANCEL × 重复消息 — a duplicate Cancel delivery is rejected without redoing
// work.
// ---------------------------------------------------------------------------

/// Delivering the same `Cancel` twice must decide the target path exactly
/// once: the first delivery undoes the running act and redoes step1 once, and
/// the rejected duplicate creates no redo work of its own.
///
/// A duplicate `Cancel` used to slip past the guard — it is checked on the
/// parent step, which stays `biz_success` after the first cancel — and created
/// a SECOND redo path (plus a second downstream act) that the client never
/// drives, wedging the run. The arm now refuses when the forward path it would
/// rewind is already terminal, and `Context::redo_task` never creates a second
/// live instance for the same `(node, prev)` slot.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_cancel_duplicate_delivery_is_rejected_cleanly() {
    bounded(
        "sch_cancel_duplicate_delivery_is_rejected_cleanly",
        sch_cancel_duplicate_delivery_is_rejected_cleanly_inner(),
    )
    .await;
}

async fn sch_cancel_duplicate_delivery_is_rejected_cleanly_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let workflow = two_step_irq_workflow();

    let s = irq_created_signal(&engine, "act1");
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // complete act1 so step1 finishes and act2 is scheduled (the cancel's
    // target path), then cancel on act1's tid: act2 is undone and step1 is
    // redone exactly once
    rt.do_action2(&pid, &act1_tid, EventAction::Next, Vars::new())
        .await
        .unwrap();
    let act2 = wait_waiting_act(&proc, "act2").await;

    let cancel = Action::new(&pid, &act1_tid, EventAction::Cancel, Vars::new());
    rt.do_action(&cancel).await.unwrap();
    assert_eq!(
        proc.task(&act2.id).unwrap().state(),
        TaskState::Cancelled,
        "the first cancel must undo the running path"
    );
    assert_eq!(
        acts_of(&proc, "act1").len(),
        2,
        "the first cancel redoes step1 exactly once"
    );

    // the duplicate delivery of the same cancel is rejected …
    let second = rt.do_action(&cancel).await;
    println!("duplicate cancel outcome: {}", outcome(&second));

    // … with no side effects: no second redo act, no phantom act2, and the
    // cancelled path stays cancelled
    assert_eq!(
        acts_of(&proc, "act1").len(),
        2,
        "the duplicate cancel must not create another redo act (acts: {:?})",
        acts_of(&proc, "act1")
            .iter()
            .map(|t| (t.id.clone(), t.state()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        acts_of(&proc, "act2").len(),
        1,
        "the duplicate cancel must not spawn a phantom act2 (acts: {:?})",
        acts_of(&proc, "act2")
            .iter()
            .map(|t| (t.id.clone(), t.state()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        proc.task(&act2.id).unwrap().state(),
        TaskState::Cancelled,
        "the duplicate cancel must not resurrect the cancelled path"
    );

    // settle through the redo path: it re-runs step1, then step2
    let redo = acts_of(&proc, "act1")
        .into_iter()
        .find(|t| t.id != act1_tid)
        .expect("the redo act must exist");
    rt.do_action2(&pid, &redo.id, EventAction::Next, Vars::new())
        .await
        .unwrap();
    let redo_act2 = wait_waiting_act(&proc, "act2").await;
    assert_ne!(
        redo_act2.id, act2.id,
        "the redo path must run a fresh act2, not the cancelled one"
    );
    rt.do_action2(&pid, &redo_act2.id, EventAction::Next, Vars::new())
        .await
        .unwrap();
    poll_until(|| async { proc.state().is_biz_success() }).await;
    assert_settled(&proc);

    sweep_until_gone(&rt, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// CANCEL × DB失败 — the store fails the cancelled act's state write once; the
// degradation is surfaced and the flow still converges exactly once.
// ---------------------------------------------------------------------------

/// The durable write of the cancelled act's `Cancelled` state fails once. The
/// cancel path degrades gracefully (the writer reports the lost write through
/// the durability barrier) while the workflow keeps moving through the redo
/// path, and the process still finishes exactly once with no duplicated or
/// stuck work.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_cancel_store_fail_degrades_and_heals_once() {
    bounded(
        "sch_cancel_store_fail_degrades_and_heals_once",
        sch_cancel_store_fail_degrades_and_heals_once_inner(),
    )
    .await;
}

async fn sch_cancel_store_fail_degrades_and_heals_once_inner() {
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

    rt.do_action2(&pid, &act1_tid, EventAction::Next, Vars::new())
        .await
        .unwrap();
    let act2 = wait_waiting_act(&proc, "act2").await;

    // the transient fault lands exactly on the cancelled act's state write
    kv.arm(&pid, &act2.id);
    rt.do_action2(&pid, &act1_tid, EventAction::Cancel, Vars::new())
        .await
        .unwrap();

    // graceful degradation, not silent corruption: the writer reports the
    // write it could not make durable
    let flush = rt.cache().flush().await;
    assert!(matches!(flush, Err(ActError::Store(_))), "{flush:?}");

    // the cancel did apply in memory: the path is undone and step1 is redone
    assert_eq!(
        proc.task(&act2.id).unwrap().state(),
        TaskState::Cancelled,
        "the cancel must decide the running act's state even when its write failed"
    );
    assert_eq!(
        acts_of(&proc, "act1").len(),
        2,
        "the cancel redoes step1 exactly once despite the store fault"
    );

    // settle through the redo path: the healed engine drives it to completion
    let redo = acts_of(&proc, "act1")
        .into_iter()
        .find(|t| t.id != act1_tid)
        .expect("the redo act must exist");
    rt.do_action2(&pid, &redo.id, EventAction::Next, Vars::new())
        .await
        .unwrap();
    let redo_act2 = wait_waiting_act(&proc, "act2").await;
    rt.do_action2(&pid, &redo_act2.id, EventAction::Next, Vars::new())
        .await
        .unwrap();
    poll_until(|| async { proc.state().is_completed() }).await;
    assert_settled(&proc);

    // no work duplicated by the fault: one original + one redo act per step
    assert_eq!(acts_of(&proc, "act1").len(), 2);
    assert_eq!(acts_of(&proc, "act2").len(), 2);

    sweep_until_gone(&rt, &pid).await;
    engine.close().await;
}

// ---------------------------------------------------------------------------
// CANCEL × 并发 — cancel racing complete on the same act tid converges.
// ---------------------------------------------------------------------------

/// `Next` (complete) and `Cancel` fired at the same act tid in the same
/// instant must converge regardless of interleaving: the act ends in exactly
/// one terminal state (`Completed`), a losing action is rejected by its
/// guards instead of corrupting, and the process always settles with exactly
/// one redo act iff the cancel won. Several rounds cover the interleavings.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_cancel_racing_next_converges_single_terminal() {
    bounded(
        "sch_cancel_racing_next_converges_single_terminal",
        sch_cancel_racing_next_converges_single_terminal_inner(),
    )
    .await;
}

async fn sch_cancel_racing_next_converges_single_terminal_inner() {
    bounded(
        "sch_cancel_racing_next_converges_single_terminal_inner",
        sch_cancel_racing_next_converges_single_terminal_inner_inner(),
    )
    .await;
}

async fn sch_cancel_racing_next_converges_single_terminal_inner_inner() {
    for round in 0..3 {
        let engine = Engine::builder().start().await.unwrap();
        let rt = engine.runtime();
        let workflow = two_step_irq_workflow();

        let s = irq_created_signal(&engine, "act1");
        let proc = rt.create_proc(&utils::longid(), &workflow);
        rt.launch(&proc).await.unwrap();
        let (pid, act1_tid) = s.recv().await;

        // the race: both actions target the same act tid at once
        let rt_next = rt.clone();
        let rt_cancel = rt.clone();
        let next = Action::new(&pid, &act1_tid, EventAction::Next, Vars::new());
        let cancel = Action::new(&pid, &act1_tid, EventAction::Cancel, Vars::new());
        let jh_next = tokio::spawn(async move { rt_next.do_action(&next).await });
        let jh_cancel = tokio::spawn(async move { rt_cancel.do_action(&cancel).await });
        let next_res = jh_next.await.unwrap();
        let cancel_res = jh_cancel.await.unwrap();
        let cancel_applied = cancel_res.is_ok();
        println!(
            "round {round}: next={} cancel={}",
            outcome(&next_res),
            outcome(&cancel_res)
        );
        // completing the waiting act is unconditional; the cancel loses
        // cleanly whenever it arrived too early for its guards
        next_res.unwrap_or_else(|e| panic!("round {round}: next must apply: {e}"));

        // the act decided by the race is in exactly one terminal state
        assert_eq!(
            proc.task(&act1_tid).unwrap().state(),
            TaskState::Completed,
            "round {round}: the raced act must end Completed exactly once"
        );

        // let the propagation settle: act2 appears in every interleaving
        // (created by step1's propagation; a winning cancel then cancels it)
        poll_until(|| async { !acts_of(&proc, "act2").is_empty() }).await;

        if cancel_applied {
            // the cancel won the race: it undid the running path and redid
            // step1 exactly once, leaving the running act2 cancelled
            assert_eq!(
                acts_of(&proc, "act1").len(),
                2,
                "round {round}: a winning cancel redoes step1 exactly once"
            );
            assert!(
                acts_of(&proc, "act2")
                    .iter()
                    .any(|t| t.state() == TaskState::Cancelled),
                "round {round}: a winning cancel must leave the running act2 cancelled"
            );
        } else {
            // the cancel was rejected by its guards: nothing undone, no redo
            assert_eq!(
                acts_of(&proc, "act1").len(),
                1,
                "round {round}: a rejected cancel must not redo step1"
            );
        }

        // settle: drive whatever still waits; the process must always finish
        let deadline = Instant::now() + WAIT;
        while !proc.state().is_completed() {
            if let Some(t) = waiting_act(&proc, "act1").filter(|t| t.id != act1_tid) {
                let _ = rt
                    .do_action2(&pid, &t.id, EventAction::Next, Vars::new())
                    .await;
            }
            if let Some(t) = waiting_act(&proc, "act2") {
                let _ = rt
                    .do_action2(&pid, &t.id, EventAction::Next, Vars::new())
                    .await;
            }
            assert!(
                Instant::now() < deadline,
                "round {round}: the process never settled (state: {})",
                proc.state()
            );
            tokio::time::sleep(TICK).await;
        }
        assert_settled(&proc);

        // deterministic counts per observed outcome, nothing extra anywhere
        let expected_acts = if cancel_applied { 2 } else { 1 };
        assert_eq!(
            acts_of(&proc, "act1").len(),
            expected_acts,
            "round {round}: act1 count must match the race outcome"
        );
        assert_eq!(
            acts_of(&proc, "act2").len(),
            expected_acts,
            "round {round}: act2 count must match the race outcome"
        );

        sweep_until_gone(&rt, &pid).await;
        engine.close().await;
    }
}

/// A `Cancel` action whose outbox record landed but whose effects were never
/// durably applied is re-applied on recovery — even though the target act is
/// already `Completed` (from the earlier `Next`), which would otherwise make
/// the terminal-state check close the record and silently drop the cancel.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_cancel() {
    bounded(
        "sch_action_recover_reapplies_cancel",
        sch_action_recover_reapplies_cancel_inner(),
    )
    .await;
}

async fn sch_action_recover_reapplies_cancel_inner() {
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
