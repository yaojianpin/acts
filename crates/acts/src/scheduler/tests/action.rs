use serde_json::json;

use crate::scheduler::Sign;
use crate::{
    Act, Action, Engine, MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    utils,
    utils::test::{USES_IRQ, USES_PARALLEL, auto_complete, create_proc},
};
use serial_test::serial;

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

/// A task marked `Sign::NEXT_PENDING` (crash after enqueue, before `next` ran)
/// is re-enqueued on recovery.
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
    let (_, tid) = s.recv().await;

    // Simulate a crash right after the action was applied and `next` was
    // enqueued, but before the queued `next` ran: mark the task pending and
    // persist it (bypassing the in-memory queue).
    let task = proc.task(&tid).unwrap();
    task.set_state(TaskState::Completed);
    task.set_sign(Sign::NEXT_PENDING);
    rt.cache().store().upsert_task(&task).unwrap();

    // Recovery re-enqueues the pending `next` idempotently.
    rt.recover_actions().unwrap();

    tx.recv().await;
    assert!(proc.state().is_success());
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
