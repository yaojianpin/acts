use crate::{
    event::ActionState,
    sch::{tests::create_proc_signal, TaskState},
    utils, Act, Action, StmtBuild, Vars, Workflow,
};
use serde_json::json;

#[tokio::test]
async fn sch_act_catch_by_any_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("aaaaaaaaaa"));
            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            rx.send(true);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_act_catch_by_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("aaaaaaaaaa"));
            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("msg1") {
            rx.send(true);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
    assert_eq!(proc.state(), TaskState::Success);
}

#[tokio::test]
async fn sch_act_catch_empty_then() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|_| Vec::new()))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("err_code".to_string(), json!("err1"));
            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Success);
}

#[tokio::test]
async fn sch_act_catch_by_err_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| {
                c.with_err("123")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1"))))
            })
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("123"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            rx.send(true);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_act_catch_by_wrong_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| {
                c.with_err("wrong_code")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1"))))
            })
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("123"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_act_catch_by_no_err_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            let state = s.do_action(&action);
            rx.send(state.is_err());
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_act_catch_as_complete() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    let p = proc.clone();

    emitter.reset();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("123"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            s.do_action(&action).unwrap();
            p.print();
        }
    });

    emitter.on_complete(move |_p| {
        rx.send(true);
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_act_catch_as_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    let p = proc.clone();
    emitter.reset();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("1"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("2"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();

            p.print();
        }
    });

    emitter.on_error(move |_p| {
        rx.send(true);
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_act_catch_as_skip() {
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_catch(|c| {
                    c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1"))))
                })
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("1"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.proc_id, &e.id, "skip", &options);
            s.do_action(&action).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("catch1").get(0).unwrap().state(),
        TaskState::Skip
    );
    assert!(proc.task_by_nid("act1").get(0).unwrap().state().is_error());
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("step2").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_catch_as_abort() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("1"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.proc_id, &e.id, "abort", &options);
            s.do_action(&action).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Abort);
}

#[tokio::test]
async fn sch_act_catch_as_submit() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch1")))))
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("1"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.proc_id, &e.id, "submit", &options);
            s.do_action(&action).unwrap();
        }
    });

    // emitter.on_complete(move |e| rx.close());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("catch1").get(0).unwrap().action_state(),
        ActionState::Submitted
    );
    assert!(proc.task_by_nid("act1").get(0).unwrap().state().is_error(),);
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_catch_as_back() {
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_catch(|c| {
                    c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("catch2"))))
                })
                .with_act(Act::req(|act| act.with_id("act2")))
        });
    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<i32>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let count = rx.data();
            if count == 1 {
                rx.close();
                return;
            }

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            s.do_action(&action).unwrap();
            rx.update(|data| *data += 1);
        }

        if e.is_key("act2") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("1"));
            options.insert("err_message".to_string(), json!("biz error"));

            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("catch2") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("to".to_string(), json!("step1"));

            let action = Action::new(&e.proc_id, &e.id, "back", &options);
            s.do_action(&action).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("catch2").get(0).unwrap().action_state(),
        ActionState::Backed
    );
    assert!(proc.task_by_nid("act2").get(0).unwrap().state().is_error());
    assert_eq!(
        proc.task_by_nid("step1").get(1).unwrap().state(),
        TaskState::Running
    );
}

#[tokio::test]
async fn sch_act_catch_and_continue() {
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_catch(|c| c.with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1")))))
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|act| act.with_id("act2")))
        });
    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("aaaaaaaaaa"));
            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            rx.send(true);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}
