use crate::{
    Act, Message, Vars, Workflow,
    package::RunningMode,
    utils::{
        self,
        test::{auto_complete, create_proc},
    },
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_block_sequence() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(
            Vars::new().with("mode", RunningMode::Sequence).with(
                "acts",
                vec![
                    Act::msg(|msg| msg.with_key("msg1")),
                    Act::msg(|msg| msg.with_key("msg2")),
                ],
            ),
        ))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_msg() {
            // std::thread::sleep(std::time::Duration::from_millis(200));
            rx.update(|data| data.push(e.inner().clone()));
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.first().unwrap().key, "msg1");
    assert_eq!(ret.get(1).unwrap().key, "msg2");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_block_parallel() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::block(
            Vars::new().with("mode", RunningMode::Parallel).with(
                "acts",
                vec![
                    Act::irq(|act| act.with_key("act1")),
                    Act::irq(|act| act.with_key("act2")),
                ],
            ),
        ))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() {
            rx.update(|data| data.push(e.inner().clone()));
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.timeout(200).await;
    proc.print();
    assert!(ret.iter().any(|iter| iter.key == "act1"));
    assert!(ret.iter().any(|iter| iter.key == "act2"));
}
