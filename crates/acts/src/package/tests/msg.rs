use crate::{
    Message, Vars, Workflow,
    utils::{
        self,
        test::{USES_MSG, auto_complete, create_proc},
    },
};
use serde_json::json;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_msg() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_msg() && e.is_type("act") {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<String>("key")
            .unwrap(),
        "msg1"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_msg_with_params_value() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_MSG, Vars::new().with("key", "msg1").with("a", 5))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        let rx = rx.clone();
        async move {
            if e.is_msg() {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<String>("key")
            .unwrap(),
        "msg1"
    );
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_msg_with_params_var() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_var("a", json!(5)).with_uses(
            USES_MSG,
            Vars::new().with("key", "msg1").with("a", "${{ a }}"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        let rx = rx.clone();
        async move {
            if e.is_msg() {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<String>("key")
            .unwrap(),
        "msg1"
    );
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<i32>("a")
            .unwrap(),
        5
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_msg_with_key() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_MSG, Vars::new().with("key", "key1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_msg() && e.is_type("act") {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(
        ret.first()
            .unwrap()
            .params()
            .unwrap()
            .get::<String>("key")
            .unwrap(),
        "key1"
    );
}
