use crate::{
    sch::{tests::create_proc_signal, TaskState},
    utils::{self, consts},
    Act, StmtBuild, Vars, Workflow,
};
use serde_json::json;

#[tokio::test]
async fn sch_step_setup_each_list() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup.add(Act::each(|each| {
                each.with_in(r#"["u1", "u2"]"#).with_then(|stmts| {
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
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
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
async fn sch_step_setup_each_var() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::set(Vars::new().with("a", ["u1", "u2"])))
                .add(Act::each(|each| {
                    each.with_in(r#"$("a")"#).with_then(|stmts| {
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
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
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
async fn sch_step_setup_each_var_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts.add(Act::each(|each| {
                each.with_in(r#"$("not_exists")"#)
                    .with_then(|stmts| stmts.add(Act::irq(|act| act.with_key("act1"))))
            }))
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_step_setup_each_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", ["u1", "u2"])))
                .add(Act::each(|each| {
                    each.with_in(
                        r#"
                        let a = $("a");
                        let b = ["u3"];
                        let c = [ "u1" ];
                        let d = [ "u3", "u4" ];

                        // result = u3
                        a.union(b).difference(c).intersection(d)
                        "#,
                    )
                    .with_then(|stmts| {
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
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
            .unwrap()
            .inputs()
            .get_value(consts::ACT_VALUE)
            .unwrap(),
        &json!("u3")
    );
}
