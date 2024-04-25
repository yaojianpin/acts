use crate::{
    event::ActionState, sch::tests::create_proc_signal, utils, Act, StmtBuild, TaskState, Workflow,
};

#[tokio::test]
async fn sch_act_cmd_submit_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("submit")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Submitted
    );
}

#[tokio::test]
async fn sch_act_cmd_sumit_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("submit").with_input("a", 5)))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Submitted
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn sch_act_cmd_submit_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::req(|req| {
                req.with_id("act1")
                    .with_on_created(|stmts| stmts.add(Act::cmd(|cmd| cmd.with_name("submit"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Submitted
    );
}

#[tokio::test]
async fn sch_act_cmd_submit_auto() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::r#if(|act| {
            act.with_on(r#"$("is_auto_submit") == null"#)
                .with_then(|acts| {
                    acts.add(Act::cmd(|cmd| {
                        cmd.with_name("submit")
                            .with_input("uid", r#"${ $("initiator") }"#)
                            .with_input("is_auto_submit", true)
                    }))
                })
                .with_else(|acts| acts.add(Act::req(|act| act.with_id("act1"))))
        }))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.task_by_nid("act1").len(), 0);
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<bool>("is_auto_submit")
            .unwrap(),
        true
    );
}

#[tokio::test]
async fn sch_act_cmd_complete_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("complete")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
}

#[tokio::test]
async fn sch_act_cmd_complete_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("complete").with_input("a", 5)))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn sch_act_cmd_complete_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::req(|req| {
                req.with_id("act1")
                    .with_on_created(|stmts| stmts.add(Act::cmd(|cmd| cmd.with_name("complete"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
}

#[tokio::test]
async fn sch_act_cmd_abort_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("abort")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Aborted
    );
    assert_eq!(proc.state(), TaskState::Abort);
}

#[tokio::test]
async fn sch_act_cmd_abort_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("abort").with_input("a", 5)))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Aborted
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn sch_act_cmd_abort_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::req(|req| {
                req.with_id("act1")
                    .with_on_created(|stmts| stmts.add(Act::cmd(|cmd| cmd.with_name("abort"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Aborted
    );
}

#[tokio::test]
async fn sch_act_cmd_error_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| {
                cmd.with_name("error").with_input("err_code", "err1")
            }))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Error
    );
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_act_cmd_error_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act({
                Act::cmd(|cmd| {
                    cmd.with_name("error")
                        .with_input("err_code", "err1")
                        .with_input("a", 5)
                })
            })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Error
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn sch_act_cmd_error_on_step_with_no_err_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("error").with_input("a", 5)))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Error
    );
    assert!(proc
        .task_by_nid("step1")
        .get(0)
        .unwrap()
        .data()
        .get::<i32>("a")
        .is_none(),);
}

#[tokio::test]
async fn sch_act_cmd_error_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::req(|req| {
                req.with_id("act1").with_on_created(|stmts| {
                    stmts.add(Act::cmd(|cmd| {
                        cmd.with_name("error").with_input("err_code", "err1")
                    }))
                })
            })
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Error
    );
}

#[tokio::test]
async fn sch_act_cmd_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("skip")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Skipped
    );
    assert!(proc.state().is_completed());
}

#[tokio::test]
async fn sch_act_cmd_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req| req.with_id("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_name("not_exist")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.task_by_nid("step1").get(0).unwrap().state().is_error(),);
    assert!(proc.state().is_error());
}
