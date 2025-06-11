use crate::{Act, Message, Workflow, scheduler::tests::create_proc_signal, utils};

#[tokio::test]
async fn sch_step_timeout_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|t| {
                t.with_on("1s").with_step(|step| {
                    step.with_id("step2")
                        .with_act(Act::msg(|msg| msg.with_key("msg1")))
                })
            })
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.is_key("msg1") {
            rx.send(true);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_step_timeout_many() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|t| {
                t.with_on("1s").with_step(|step| {
                    step.with_id("step2")
                        .with_act(Act::msg(|msg| msg.with_key("msg1")))
                })
            })
            .with_timeout(|t| {
                t.with_on("2s").with_step(|step| {
                    step.with_id("step3")
                        .with_act(Act::msg(|msg| msg.with_key("msg2")))
                })
            })
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.is_key("msg1") {
            rx.update(|data| data.push(e.inner().clone()));
        }

        if e.is_key("msg2") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 2)
}
