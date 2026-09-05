use serde_json::json;

use crate::MessageState;
use crate::event::EventAction;
use crate::{
    Message, Vars, Workflow,
    scheduler::TaskState,
    utils::{
        self,
        test::{USES_ACTION, USES_IRQ, USES_MSG, USES_SET, auto_complete, create_proc},
    },
};

use serial_test::serial;

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
