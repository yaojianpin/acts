use crate::{
    scheduler::{tests::create_proc_signal, TaskState},
    utils, Act, StmtBuild, Vars, Workflow,
};

#[tokio::test]
async fn sch_act_if_true() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", 10)))
                .add(Act::r#if(|cond| {
                    cond.with_on(r#"$("a") > 0"#).with_then(|stmts| {
                        stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                    })
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
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_if_false() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", 10)))
                .add(Act::r#if(|cond| {
                    cond.with_on(r#"$("a") < 0"#)
                        .with_then(|stmts| stmts.add(Act::irq(|act| act.with_key("act1"))))
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
async fn sch_act_if_null_value() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup.add(Act::r#if(|cond| {
                cond.with_on(r#"$("a") == null"#).with_then(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                })
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
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_if_else() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", 10)))
                .add(Act::r#if(|cond| {
                    cond.with_on(r#"$("a") < 0"#).with_else(|stmts| {
                        stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                    })
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
