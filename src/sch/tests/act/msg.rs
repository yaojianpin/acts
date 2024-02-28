use crate::{sch::tests::create_proc, utils, Act, StmtBuild, Workflow};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_act_msg() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_id("msg1"))))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_act_msg_with_inputs() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5))))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_act_msg_with_inputs_var() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!(5))
            .with_setup(|setup| {
                setup.add(Act::msg(|msg| {
                    msg.with_id("msg1").with_input("a", r#"${ env.get("a") }"#)
                }))
            })
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}


#[tokio::test]
async fn sch_act_msg_with_key() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_id("msg1").with_key("key1"))))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "key1");
}