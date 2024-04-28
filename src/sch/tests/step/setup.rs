use serde_json::json;

use crate::{
    sch::{tests::create_proc_signal, TaskState},
    utils::{self, consts},
    Act, Message, StmtBuild, Vars, Workflow,
};

#[tokio::test]
async fn sch_step_setup_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.get(0).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_setup_req() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| stmts.add(Act::req(|req: crate::Req| req.with_id("act1"))))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("req") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.get(0).unwrap().key, "act1");
}

#[tokio::test]
async fn sch_step_setup_set() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| stmts.add(Act::set(Vars::new().with("a", 10))))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_step_setup_expose() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| stmts.add(Act::expose(Vars::new().with("a", 10))))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.data().get::<i32>("a").unwrap(), 10);
}

#[tokio::test]
async fn sch_step_setup_if_true() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::r#if(|c| {
                    c.with_on(r#"$("a") > 0"#)
                        .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
                }))
            })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.task_by_nid("act1").len(), 1);
}

#[tokio::test]
async fn sch_step_setup_if_false() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::r#if(|c| {
                    c.with_on(r#"$("a") < 0"#)
                        .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
                }))
            })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.task_by_nid("act1").len(), 0);
}

#[tokio::test]
async fn sch_step_setup_each() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::each(|each| {
                    each.with_in(r#"["u1", "u2"]"#)
                        .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
                }))
            })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act1")
            .get(0)
            .unwrap()
            .inputs()
            .get_value(consts::ACT_VALUE)
            .unwrap(),
        &json!("u1")
    );
    assert_eq!(
        proc.task_by_nid("act1")
            .get(1)
            .unwrap()
            .inputs()
            .get_value(consts::ACT_VALUE)
            .unwrap(),
        &json!("u2")
    );
}

#[tokio::test]
async fn sch_step_setup_chain() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::chain(|act| {
                    act.with_in(r#"["u1", "u2"]"#)
                        .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
                }))
            })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<String>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
            rx.update(|data| data.push(e.inputs.get::<String>(consts::ACT_VALUE).unwrap()));
            e.do_action(&e.proc_id, &e.id, consts::EVT_NEXT, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret, ["u1", "u2"]);
}

#[tokio::test]
async fn sch_step_setup_pack() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::block(|act| {
                    act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
                        .with_next(|act| {
                            act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg2"))))
                        })
                }))
            })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<String>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_type("msg") {
            rx.update(|data| data.push(e.key.clone()));
            e.do_action(&e.proc_id, &e.id, consts::EVT_NEXT, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret, ["msg1", "msg2"]);
}

#[tokio::test]
async fn sch_step_setup_cmd() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| stmts.add(Act::cmd(|act| act.with_name(consts::EVT_NEXT))))
    });

    workflow.print();
    let (proc, scher, .., tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Completed
    );
}
