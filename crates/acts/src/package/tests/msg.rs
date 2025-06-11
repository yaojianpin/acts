use crate::{
    Act, Message, StmtBuild, Workflow,
    utils::{self, test::create_proc_signal},
};
use serde_json::json;

#[tokio::test]
async fn pack_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_key("msg1"))))
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
async fn pack_msg_with_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_key("msg1").with_input("a", 5))))
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
    assert_eq!(ret.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn pack_msg_with_inputs_var() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!(5))
            .with_setup(|setup| {
                setup.add(Act::msg(|msg| {
                    msg.with_key("msg1").with_input("a", r#"{{ a }}"#)
                }))
            })
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
    assert_eq!(ret.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn pack_msg_with_key() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_key("msg1").with_key("key1"))))
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
    assert_eq!(ret.first().unwrap().key, "key1");
}
