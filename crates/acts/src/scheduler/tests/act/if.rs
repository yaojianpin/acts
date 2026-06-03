use crate::{
    Act, TaskState, Vars, Workflow,
    utils::{
        self,
        test::{auto_complete, create_proc},
    },
};

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_true() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", 10)))
            .with_act(Act::msg(|act| {
                act.with_if(r#"a > 0"#).with_key("msg1").with_id("msg1")
            }))
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.is_msg() {
            rx.close();
        }
    });
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_false() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", 10)))
            .with_act(Act::msg(|act| {
                act.with_if(r#"a < 0"#).with_key("msg1").with_id("msg1")
            }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());

    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_null_value() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::msg(|act| {
            act.with_if(r#"$get("not_exists") == null"#)
                .with_key("msg1")
                .with_id("msg1")
        }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.is_msg() {
            println!("aaaaaaaaaaaaa: {e:?}");
            rx.close();
        }
    });
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().node().key(),
        "msg1"
    );
}
