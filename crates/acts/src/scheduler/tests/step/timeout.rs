use crate::{
    Act, Message, Workflow,
    utils::{
        self,
        test::{auto_complete, create_proc},
    },
};

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_one() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(Act::msg(|msg| {
                msg.with_key("msg1").with_if(r#"$cost() >= 1000"#)
            }))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.is_key("msg1") {
            rx.send(true);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_many() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(Act::msg(|msg| {
                msg.with_key("msg1").with_if(r#"$cost() >= 1000"#)
            }))
            .with_timeout(Act::msg(|msg| {
                msg.with_key("msg2").with_if(r#"$cost() >= 2000"#)
            }))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("msg1") {
            rx.update(|data| data.push(e.inner().clone()));
        }

        if e.is_key("msg2") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 2)
}
