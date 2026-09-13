use serde_json::json;

use crate::MessageState;
use crate::event::EventAction;
use crate::{
    Message, Vars, Workflow,
    scheduler::{ActTask, Sign, TaskState},
    utils::{
        self,
        test::{USES_ACTION, USES_IRQ, USES_MSG, USES_SET, auto_complete, create_proc},
    },
};

use serial_test::serial;
use std::time::Duration;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_msg() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_msg() && e.is_type("act") {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<String>("key")
            .unwrap(),
        "msg1"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_irq() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);

    let channel = engine.channel();
    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_irq() && e.is_type("act") {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<String>("key")
            .unwrap(),
        "act1"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_set() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(0))
            .with_uses(USES_SET, Vars::new().with("a", 10))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_if_true() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_if(r#"a > 0"#)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            println!("message: {e:?}");
            if e.is_type("act") {
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .state()
            .is_running()
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_if_false() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_if(r#"a < 0"#)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            println!("message: {e:?}");
            if e.is_type("act") {
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_action() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10)).with_id("step1").with_uses(
            USES_ACTION,
            Vars::new()
                .with("action", EventAction::Next)
                .with("options", json!({})),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_action_and_then_branch() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_uses(
                USES_ACTION,
                Vars::new()
                    .with("action", EventAction::Next)
                    .with("options", json!({ "a": 0 })),
            )
            .with_branch(|b| b.with_id("b1").with_if("a > 0"))
            .with_branch(|b| b.with_id("b2").with_if("a == 0"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert_eq!(
        proc.task_by_nid("b2").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_irq_and_then_branch() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_branch(|b| b.with_id("b1").with_if("a > 0"))
            .with_branch(|b| b.with_id("b2").with_if("a == 0"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let rt = engine.runtime();
    engine.channel().on_message(move |e| {
        let rt = rt.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new().with("a", 0))
                    .await
                    .unwrap();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert_eq!(
        proc.task_by_nid("b2").first().unwrap().state(),
        TaskState::Completed
    );
}

/// A step with `uses` completes only once its act child's own `next` has
/// propagated the child's outputs into the step.
///
/// The child's terminal state is written by whichever job applies its action
/// (the action package writes `Submitted`, a client ack writes `Completed`)
/// while the child's `next` is dispatched separately, so the step's `next` can
/// run in between. Counting the terminal state alone then completes the step —
/// and the whole workflow — with the child's outputs missing; the step must
/// wait for the child's [`Sign::NEXT_COMPLETE`] marker, which only the child's
/// `next` sets.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_waits_for_child_next_before_complete() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    rt.launch(&proc).await.unwrap();

    // the irq act stays `Interrupt`, so the step is quiescent while we drive
    // its `next` by hand — no lane job can race these calls
    let mut step = None;
    for _ in 0..100 {
        if let Some(found) = proc.task_by_nid("step1").first().cloned()
            && !proc.children(&found.id).is_empty()
        {
            step = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let step = step.expect("step task and its uses act task must exist");
    let child = proc.children(&step.id).first().unwrap().clone();
    let ctx = proc.create_context(&step);
    ctx.set_task(&step);

    // the action's state write: the child is terminal, but its `next` (which
    // propagates its outputs) has not run yet
    child.set_state(TaskState::Submitted);
    for _ in 0..3 {
        ActTask::next(&step, &ctx).await.unwrap();
    }
    assert!(
        !step.is_sign(Sign::USES_COMPLETE),
        "a terminal uses child whose `next` has not run must not mark the step's uses complete"
    );
    assert!(
        !step.state().is_completed(),
        "the step must not complete before its uses child propagated"
    );

    // the child's `next` propagated its outputs into the step
    child.set_sign(Sign::NEXT_COMPLETE);
    for _ in 0..4 {
        ActTask::next(&step, &ctx).await.unwrap();
        if step.state().is_completed() {
            break;
        }
    }
    assert!(step.is_sign(Sign::USES_COMPLETE));
    assert!(
        step.state().is_completed(),
        "the step completes once its uses child propagated"
    );
}
