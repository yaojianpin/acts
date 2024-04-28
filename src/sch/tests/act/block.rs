use crate::{
    sch::{tests::create_proc_signal, TaskState},
    utils, Act, StmtBuild, Workflow,
};

#[tokio::test]
async fn sch_act_block_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(|act| {
            act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
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
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_act_block_next() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(|act| {
            act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
                .with_next(|act| {
                    act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg2"))))
                })
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
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_act_block_acts() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(|act| {
            act.with_id("pack1").with_acts(|stmts| {
                stmts
                    .add(Act::msg(|msg| msg.with_id("msg1")))
                    .add(Act::req(|act| act.with_id("act1")))
            })
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("act1") && e.is_state("created") {
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
        proc.task_by_nid("pack1").get(0).unwrap().state(),
        TaskState::Running
    );
}
