use crate::{
    Message, Vars, Workflow,
    utils::{
        self,
        test::{USES_IRQ, USES_MSG, auto_complete, create_proc},
    },
};

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_one() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|step| {
                step.with_id("timout1")
                    .with_if(r#"$cost() >= 1000"#)
                    .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_params_key("msg1") {
                rx.send(true);
            }
        }
    });

    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_many() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|step| {
                step.with_id("timeout1")
                    .with_if(r#"$cost() >= 1000 && $cost() < 2000"#)
                    .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
            })
            .with_timeout(|step| {
                step.with_id("timeout2")
                    .with_if(r#"$cost() >= 2000 && $cost() < 3000"#)
                    .with_uses(USES_MSG, Vars::new().with("key", "msg2"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    let channel = engine.channel();
    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            // println!("message: {e:?}");
            if e.is_params_key("msg1") {
                rx.update(|data| data.push(e.inner().clone()));
            }

            if e.is_params_key("msg2") {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });

    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 2)
}
