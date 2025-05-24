use crate::event::EventAction;
use crate::{
    Act, MessageState, Vars, Workflow,
    utils::{self, test::create_proc_signal},
};

#[tokio::test]
async fn sch_act_run_in_order() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
            .with_act(Act::irq(|act| act.with_key("act2")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<(String, i64)>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            rx.update(|data| data.push((e.key.clone(), e.start_time)));
            std::thread::sleep(std::time::Duration::from_millis(1000));
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret.len(), 2);
    assert!(ret.get(1).unwrap().1 - ret.first().unwrap().1 >= 1000);
}
