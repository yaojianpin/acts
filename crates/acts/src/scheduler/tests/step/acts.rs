use serde_json::json;

use crate::event::EventAction;
use crate::{
    Act, Message, Vars, Workflow,
    scheduler::{TaskState, tests::create_proc_signal},
    utils,
};

#[tokio::test]
async fn sch_step_acts_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::msg(|msg| msg.with_key("msg1")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
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
async fn sch_step_acts_req() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req| req.with_key("act1")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "act1");
}

/// acts.core.set will update the vars in the step
#[tokio::test]
async fn sch_step_acts_set() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(0))
            .with_act(Act::set(Vars::new().with("a", 10)))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_step_acts_if_true() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_act(Act::irq(|act| act.with_if(r#"a > 0"#).with_key("act1")).with_id("act1"))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_type("act") {
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
}

#[tokio::test]
async fn sch_step_acts_if_false() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_act(Act::irq(|act| act.with_if(r#"a < 0"#).with_key("act1")).with_id("act1"))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_type("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[tokio::test]
async fn sch_step_acts_action() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!(10))
            .with_id("step1")
            .with_act(Act::action(Vars::new().with("action", EventAction::Next)))
    });

    workflow.print();
    let (proc, scher, .., tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}
