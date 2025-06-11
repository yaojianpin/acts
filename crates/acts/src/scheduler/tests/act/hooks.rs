use crate::ActEvent;
use crate::event::EventAction;
use crate::utils::test::create_proc_signal_with_auto_clomplete;
use crate::{
    Act, Message, MessageState, StmtBuild, Vars, Workflow, scheduler::tests::create_proc_signal,
    utils,
};

#[tokio::test]
async fn sch_act_hooks_created() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_setup(|stmts| {
                stmts.add(Act::msg(|msg| {
                    msg.with_on(ActEvent::Created).with_key("msg1")
                }))
            }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_act_hooks_completed() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_setup(|stmts| {
                stmts.add(Act::msg(|act| {
                    act.with_on(ActEvent::Completed).with_key("msg1")
                }))
            }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}
