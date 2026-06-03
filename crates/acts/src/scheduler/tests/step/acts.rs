use serde_json::json;

use crate::event::EventAction;
use crate::utils::test::auto_complete;
use crate::{
    Act, Message, Vars, Workflow,
    scheduler::TaskState,
    utils::{self, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_acts_msg() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::msg(|msg| msg.with_key("msg1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_acts_req() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);

    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "act1");
}

/// acts.core.set will update the vars in the step
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_acts_set() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(0))
            .with_act(Act::set(Vars::new().with("a", 10)))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).unwrap();
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
async fn sch_step_acts_if_true() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_act(Act::irq(|act| act.with_if(r#"a > 0"#).with_key("act1")).with_id("act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_type("act") {
            rx.close();
        }
    });
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_acts_if_false() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_act(Act::irq(|act| act.with_if(r#"a < 0"#).with_key("act1")).with_id("act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_type("act") {
            rx.close();
        }
    });
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_acts_action() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_act(Act::action(Vars::new().with("action", EventAction::Next)))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}
