use serde_json::json;

use crate::{
    TaskState, Vars, Workflow,
    event::EventAction,
    utils,
    utils::test::{USES_ACTION, auto_complete, create_proc},
};

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_submit_on_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_ACTION, Vars::new().with("action", "submit"))
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
        step.with_id("step1").with_uses(
            USES_ACTION,
            Vars::new().with("action", "submit").with(
                "options",
                json!({
                    "a": 5
                }),
            ),
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
        step.with_id("step1")
            .with_if(r#"$get("is_auto_submit") == null"#)
            .with_uses(
                USES_ACTION,
                Vars::new().with("action", "submit").with(
                    "options",
                    json!({
                        "is_auto_submit": true
                    }),
                ),
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
            .with_uses(USES_ACTION, Vars::new().with("action", EventAction::Next))
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
        step.with_id("step1").with_uses(
            USES_ACTION,
            Vars::new().with("action", EventAction::Next).with(
                "options",
                json!({
                    "a": 5
                }),
            ),
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
            .with_uses(USES_ACTION, Vars::new().with("action", "abort"))
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
        step.with_id("step1").with_uses(
            USES_ACTION,
            Vars::new().with("action", "abort").with(
                "options",
                json!({
                    "a": 5
                }),
            ),
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
        step.with_id("step1").with_uses(
            USES_ACTION,
            Vars::new().with("action", "error").with(
                "options",
                json!({
                    "ecode": "err1"
                }),
            ),
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
        TaskState::Error
    );
    assert!(proc.state().is_error());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_error_on_step_with_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_ACTION,
            Vars::new().with("action", "error").with(
                "options",
                json!({
                    "ecode": "err1",
                    "a": 5
                }),
            ),
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
        step.with_id("step1").with_uses(
            USES_ACTION,
            Vars::new().with("action", "error").with(
                "options",
                json!({
                    "a": 5
                }),
            ),
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
        proc.task_by_uses(USES_ACTION).first().unwrap().state(),
        TaskState::Error
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_skip_on_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_ACTION, Vars::new().with("action", "skip"))
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
            .with_uses(USES_ACTION, Vars::new().with("action", "not_exist"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).unwrap();
    tx.timeout(100).await;
    proc.print();
    assert!(
        proc.task_by_uses(USES_ACTION)
            .first()
            .unwrap()
            .state()
            .is_error(),
    );
}
