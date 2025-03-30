use crate::event::EventAction;
use crate::{sch::tests::create_proc_signal, utils, Act, StmtBuild, TaskState, Workflow};

#[tokio::test]
async fn sch_act_cmd_submit_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key("submit")))
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
async fn sch_act_cmd_sumit_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key("submit").with_input("a", 5)))
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
async fn sch_act_cmd_submit_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::new()
                .with_act("irq")
                .with_key("act1")
                .with_id("act1")
                .with_setup(|stmts| stmts.add(Act::cmd(|cmd| cmd.with_key("submit")))),
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
async fn sch_act_cmd_submit_auto() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::r#if(|act| {
            act.with_on(r#"$("is_auto_submit") == null"#)
                .with_then(|acts| {
                    acts.add(Act::cmd(|cmd| {
                        cmd.with_key("submit")
                            .with_input("uid", r#"${ $("initiator") }"#)
                            .with_input("is_auto_submit", true)
                    }))
                })
                .with_else(|acts| acts.add(Act::irq(|act| act.with_key("act1"))))
        }))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.task_by_nid("act1").len(), 0);
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Submitted
    );
    assert!(proc
        .task_by_nid("step1")
        .first()
        .unwrap()
        .data()
        .get::<bool>("is_auto_submit")
        .unwrap());
}

#[tokio::test]
async fn sch_act_cmd_complete_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key(EventAction::Next.as_ref())))
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
async fn sch_act_cmd_complete_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| {
                cmd.with_key(EventAction::Next.as_ref()).with_input("a", 5)
            }))
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
async fn sch_act_cmd_complete_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::new()
                .with_act("irq")
                .with_id("act1")
                .with_key("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::cmd(|cmd| cmd.with_key(EventAction::Next.as_ref())))
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
async fn sch_act_cmd_abort_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key("abort")))
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
async fn sch_act_cmd_abort_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key("abort").with_input("a", 5)))
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
async fn sch_act_cmd_abort_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::irq(|req| req.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| stmts.add(Act::cmd(|cmd| cmd.with_key("abort"))))
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
async fn sch_act_cmd_error_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| {
                cmd.with_key("error").with_input("ecode", "err1")
            }))
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
async fn sch_act_cmd_error_on_step_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act({
                Act::cmd(|cmd| {
                    cmd.with_key("error")
                        .with_input("ecode", "err1")
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
async fn sch_act_cmd_error_on_step_with_no_err_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| {
                cmd.with_key(EventAction::Error.as_ref()).with_input("a", 5)
            }))
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
    assert!(proc
        .task_by_nid("step1")
        .first()
        .unwrap()
        .data()
        .get::<i32>("a")
        .is_none(),);
}

#[tokio::test]
async fn sch_act_cmd_error_on_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::irq(|req| req.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_created(|stmts| {
                        stmts.add(Act::cmd(|cmd| {
                            cmd.with_key("error").with_input("ecode", "err1")
                        }))
                    }))
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
        TaskState::Error
    );
}

#[tokio::test]
async fn sch_act_cmd_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key("skip")))
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
async fn sch_act_cmd_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
            .with_act(Act::cmd(|cmd| cmd.with_key("not_exist")))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc
        .task_by_nid("step1")
        .first()
        .unwrap()
        .state()
        .is_error(),);
    assert!(proc.state().is_error());
}
