use serde_json::json;

use crate::Vars;
use crate::event::EventAction;
use crate::utils::test::auto_complete;
use crate::{Act, TaskState, Workflow, utils, utils::test::create_proc};

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_submit_on_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "submit")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Submitted
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_sumit_on_step_with_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "submit").with(
                "options",
                json!({
                    "a": 5
                }),
            )))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Submitted
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_submit_auto() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::action(Vars::new().with("action", "submit").with(
                "options",
                json!({
                    "is_auto_submit": true
                }),
            ))
            .with_if(r#"$get("is_auto_submit") == null"#),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Submitted
    );
    assert!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<bool>("is_auto_submit")
            .unwrap()
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_complete_on_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", EventAction::Next)))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_action_complete_on_step_with_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(
                Vars::new().with("action", EventAction::Next).with(
                    "options",
                    json!({
                        "a": 5
                    }),
                ),
            ))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_action_abort_on_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "abort")))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Aborted
    );
    assert_eq!(proc.state(), TaskState::Aborted);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_abort_on_step_with_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "abort").with(
                "options",
                json!({
                    "a": 5
                }),
            )))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Aborted
    );

    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_error_on_step_normal() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "error").with(
                "options",
                json!({
                    "ecode": "err1"
                }),
            )))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Error
    );
    assert!(proc.state().is_error());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_error_on_step_with_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "error").with(
                "options",
                json!({
                    "ecode": "err1",
                    "a": 5
                }),
            )))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Error
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_error_on_step_with_no_err_code() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            // no error code provided, cannot execute the action, should not cause the step to change the state
            Act::action(Vars::new().with("action", "error").with(
                "options",
                json!({
                    "a": 5
                }),
            ))
            .with_id("act1"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).unwrap();
    tx.timeout(200).await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Error
    );
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Error
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_skip_on_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "skip")))
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
        TaskState::Skipped
    );
    assert!(proc.state().is_completed());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_not_exist() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "not_exist")).with_id("act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).unwrap();
    tx.timeout(100).await;
    proc.print();
    assert!(proc.task_by_nid("act1").first().unwrap().state().is_error(),);
}
