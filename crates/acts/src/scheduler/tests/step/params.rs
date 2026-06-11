use crate::{
    Message, MessageState, Vars, Workflow,
    event::EventAction,
    utils::{
        self,
        test::{USES_IRQ, USES_MSG, USES_SET, auto_complete, create_proc},
    },
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_basic() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vars::new()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1")
            && e.is_state(MessageState::Created)
            && let Some(params) = e.params()
        {
            rx.send(params);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    // verify params contains the key we set
    assert_eq!(ret.get::<String>("key").unwrap(), "act1");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_multiple() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_IRQ,
            Vars::new()
                .with("key", "act1")
                .with("priority", 5)
                .with("label", "important"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vars::new()).double();
    auto_complete(&engine, &rx);
    let rt2 = rt.clone();
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            if let Some(params) = e.params() {
                rx.send(params);
            }
            // complete the act so workflow can finish
            rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    // verify all params are accessible
    assert_eq!(ret.get::<String>("key").unwrap(), "act1");
    assert_eq!(ret.get::<i32>("priority").unwrap(), 5);
    assert_eq!(ret.get::<String>("label").unwrap(), "important");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_from_node() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SET,
            Vars::new()
                .with("action_name", "do_something")
                .with("value", 42),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(proc.state(), crate::scheduler::TaskState::Completed);
    // verify step params are accessible from the task node
    let tasks = proc.task_by_nid("step1");
    let task = tasks.first().unwrap();
    let params = task.node().params();
    assert!(params.is_object());
    assert_eq!(
        params.get("action_name").unwrap().as_str().unwrap(),
        "do_something"
    );
    assert_eq!(params.get("value").unwrap().as_i64().unwrap(), 42);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_empty() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_SET, Vars::new().with("a", 1))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(proc.state(), crate::scheduler::TaskState::Completed);
    // verify step has the uses set but params field itself was set via with_uses
    let tasks = proc.task_by_nid("step1");
    let task = tasks.first().unwrap();
    let params = task.node().params();
    assert!(params.is_object());
    assert_eq!(params.get("a").unwrap().as_i64().unwrap(), 1);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_with_vars() {
    let workflow = Workflow::new().with_var("count", 10).with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1").with("total", 100))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vars::new()).double();
    auto_complete(&engine, &rx);
    let rt2 = rt.clone();
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            if let Some(params) = e.params() {
                rx.send(params);
            }
            // complete the act so workflow can finish
            rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    // verify params contain both key and total
    assert_eq!(ret.get::<String>("key").unwrap(), "act1");
    assert_eq!(ret.get::<i32>("total").unwrap(), 100);
    // verify workflow var is accessible from task data
    let tasks = proc.task_by_nid("step1");
    let task = tasks.first().unwrap();
    assert_eq!(task.data().get::<i32>("count").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_msg() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_MSG,
            Vars::new().with("key", "msg1").with("channel", "main"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_msg() && e.is_type("act") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    let params = ret.first().unwrap().params().unwrap();
    assert_eq!(params.get::<String>("key").unwrap(), "msg1");
    assert_eq!(params.get::<String>("channel").unwrap(), "main");
}
