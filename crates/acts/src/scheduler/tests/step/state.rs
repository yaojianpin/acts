use crate::{
    MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    utils::{self, test::USES_IRQ, test::auto_complete, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_state_ready() {
    // Step task transitions to Ready state during init, observable via on_task
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    rt.emitter().on_task(move |e| {
        if e.node().id == "step1" && e.state() == TaskState::Ready {
            tx2.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_state_pending() {
    // Branch with with_need sets dependent branch to Pending
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("true")
                    .with_name("branch 1")
                    .with_step(|step| {
                        step.with_id("step11")
                            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                    })
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if("true")
                    .with_name("branch 2")
                    .with_need("b1")
                    .with_step(|step| step.with_id("step21"))
            })
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    let proc2 = proc.clone();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            // branch b1 should be Running and b2 should be Pending
            if let Some(task) = proc2.task_by_nid("b2").first()
                && task.state() == TaskState::Pending {
                    tx2.send(true);
                }
            // Complete the act so workflow can finish
            rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_state_running() {
    // Step task stays Running while its IRQ act is being processed
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    let proc2 = proc.clone();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            // step task should be Running while act is being processed
            if let Some(task) = proc2.task_by_nid("step1").first()
                && task.state() == TaskState::Running {
                    tx2.send(true);
                }
            // Complete the act so workflow can finish
            rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_state_completed() {
    // Simple step completes successfully
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    // step task should be Completed
    let tasks = proc.task_by_nid("step1");
    assert!(!tasks.is_empty());
    assert_eq!(tasks.first().unwrap().state(), TaskState::Completed);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_state_aborted() {
    // When an IRQ act is aborted, the act task goes to Aborted
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    let proc2 = proc.clone();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set("uid", "u1");
            rt2.do_action2(&e.pid, &e.tid, EventAction::Abort, options)
                .unwrap();
        }
        if e.is_params_key("act1") && e.is_state(MessageState::Aborted) {
            // act task should be Aborted
            let tasks = proc2.task_by_nid(&e.nid);
            if let Some(task) = tasks.first()
                && task.state() == TaskState::Aborted {
                    tx2.send(true);
                }
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_state_error() {
    // When an IRQ act receives Error action, the act task goes to Error
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    let proc2 = proc.clone();
    engine.channel().on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set("ecode", "err1");
            rt2.do_action2(&e.pid, &e.tid, EventAction::Error, options)
                .unwrap();
        }
        if e.is_params_key("act1") && e.is_state(MessageState::Error) {
            // act task should be Error
            let tasks = proc2.task_by_nid(&e.nid);
            if let Some(task) = tasks.first()
                && task.state() == TaskState::Error {
                    tx2.send(true);
                }
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}
