use serde_json::json;

use crate::Vars;
use crate::event::EventAction;
use crate::{Act, StmtBuild, TaskState, Workflow, utils, utils::test::create_proc_signal};

#[tokio::test]
async fn pack_action_submit_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "submit")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Submitted
    );
}

#[tokio::test]
async fn pack_action_sumit_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "submit").with(
                "options",
                json!({
                    "a": 5
                }),
            )))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
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

#[tokio::test]
async fn pack_action_submit_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("act1").with_id("act1")).with_setup(|stmts| {
                stmts.add(
                    Act::action(Vars::new().with("action", "submit"))
                        .with_on(crate::ActEvent::Created),
                )
            }),
        )
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Submitted
    );
}

#[tokio::test]
async fn pack_action_submit_auto() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
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

#[tokio::test]
async fn pack_action_complete_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", EventAction::Next)))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn pack_action_complete_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
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

#[tokio::test]
async fn pack_action_complete_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_id("act1").with_key("act1")).with_setup(|stmts| {
                stmts.add(
                    Act::action(Vars::new().with("action", EventAction::Next))
                        .with_on(crate::ActEvent::Created),
                )
            }),
        )
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn pack_action_abort_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "abort")))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Aborted
    );
    assert_eq!(proc.state(), TaskState::Aborted);
}

#[tokio::test]
async fn pack_action_abort_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
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

#[tokio::test]
async fn pack_action_abort_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::irq(|act| act.with_id("act1")).with_setup(|stmts| {
                stmts.add(
                    Act::action(Vars::new().with("action", "abort"))
                        .with_on(crate::ActEvent::Created),
                )
            })
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Aborted
    );
}

#[tokio::test]
async fn pack_action_error_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Error
    );
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn pack_action_error_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
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

#[tokio::test]
async fn pack_action_error_on_step_with_no_err_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.timeout(200).await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Running
    );
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Error
    );
}

#[tokio::test]
async fn pack_action_error_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_id("act1")).with_setup(|smts| {
                smts.add(
                    Act::action(Vars::new().with("action", "error").with(
                        "options",
                        json!({
                            "ecode": "err1"
                        }),
                    ))
                    .with_on(crate::ActEvent::Created),
                )
            }))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Error
    );
}

#[tokio::test]
async fn pack_action_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "skip")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert!(proc.state().is_completed());
}

#[tokio::test]
async fn pack_action_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::action(Vars::new().with("action", "not_exist")).with_id("act1"))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.task_by_nid("act1").first().unwrap().state().is_error(),);
    assert!(proc.state().is_error());
}
