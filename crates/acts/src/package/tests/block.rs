use crate::{
    Act, Message, Vars, Workflow,
    package::RunningMode,
    utils::{self, test::create_proc_signal},
};

#[tokio::test]
async fn pack_block_sequence() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            // std::thread::sleep(std::time::Duration::from_millis(200));
            rx.update(|data| data.push(e.inner().clone()));
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.first().unwrap().key, "msg1");
    assert_eq!(ret.get(1).unwrap().key, "msg2");
}

#[tokio::test]
async fn pack_block_parallel() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() {
            rx.update(|data| data.push(e.inner().clone()));
        }
    });
    scher.launch(&proc);
    let ret = tx.timeout(200).await;
    proc.print();
    assert!(ret.iter().any(|iter| iter.key == "act1"));
    assert!(ret.iter().any(|iter| iter.key == "act2"));
}
