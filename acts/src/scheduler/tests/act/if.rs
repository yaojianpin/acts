use crate::{
    Act, StmtBuild, TaskState, Vars, Workflow, scheduler::tests::create_proc_signal, utils,
};

#[tokio::test]
async fn sch_act_if_true() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", 10)))
                .add(Act::msg(|act| {
                    act.with_if(r#"a > 0"#).with_key("msg1").with_id("msg1")
                }))
                .add(Act::irq(|act| act.with_key("act1")))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_if_false() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", 10)))
                .add(Act::msg(|act| {
                    act.with_if(r#"a < 0"#).with_key("msg1").with_id("msg1")
                }))
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[tokio::test]
async fn sch_act_if_null_value() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup.add(Act::msg(|act| {
                act.with_if(r#""$("a") == null"#)
                    .with_key("msg1")
                    .with_id("msg1")
            }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().node().key(),
        "msg1"
    );
}
