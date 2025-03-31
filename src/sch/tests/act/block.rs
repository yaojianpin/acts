use crate::{
    sch::{tests::create_proc_signal, TaskState},
    utils, Act, MessageState, StmtBuild, Workflow,
};

#[tokio::test]
async fn sch_act_block_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(|act| {
            act.with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg1"))))
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("msg1") {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_act_block_next() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(|act| {
            act.with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg1"))))
                .with_next(|act| act.with_act("msg").with_key("msg2").with_id("msg2"))
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("msg2") {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_act_block_acts() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::block(|act| {
                act.with_then(|stmts| {
                    stmts
                        .add(Act::msg(|msg| msg.with_key("msg1")).with_id("msg1"))
                        .add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                })
            })
            .with_id("pack1"),
        )
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("act1") && e.is_state(MessageState::Created) {
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
        proc.task_by_nid("pack1").first().unwrap().state(),
        TaskState::Running
    );
}
