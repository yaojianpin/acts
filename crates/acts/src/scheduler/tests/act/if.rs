use crate::{
    Act, TaskState, Vars, Workflow,
    package::RunningMode,
    utils::{
        self,
        test::{USES_BLOCK, auto_complete, create_proc},
    },
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_true() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_BLOCK,
            Vars::new().with("mode", RunningMode::Sequence).with(
                "acts",
                vec![
                    Act::set(Vars::new().with("a", 10)),
                    Act::msg(|act| act.with_id("msg1")),
                ],
            ),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_msg() {
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
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
        step.with_id("step1").with_uses(
            USES_BLOCK,
            Vars::new().with("mode", RunningMode::Sequence).with(
                "acts",
                vec![
                    Act::set(Vars::new().with("a", 10)),
                    Act::msg(|act| act.with_id("msg1").with_if("false")),
                ],
            ),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;

    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_null_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_BLOCK,
            Vars::new().with("mode", RunningMode::Sequence).with(
                "acts",
                vec![
                    Act::set(Vars::new().with("a", 10)),
                    Act::msg(|act| act.with_id("msg1").with_if("null")),
                ],
            ),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_msg() {
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("msg1").first().unwrap().state(),
        TaskState::Error
    );
}
