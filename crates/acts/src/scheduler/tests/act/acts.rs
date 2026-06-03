use crate::event::EventAction;
use crate::utils::test::auto_complete;
use crate::{
    Act, MessageState, Vars, Workflow,
    utils::{self, test::create_proc},
};
use serde_json::json;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_run_in_order() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
            .with_act(Act::irq(|act| act.with_key("act2")))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(Vec::<(String, i64)>::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    let rt = engine.runtime();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            rx.update(|data| data.push((e.key.clone(), e.start_time)));
            std::thread::sleep(std::time::Duration::from_millis(1000));
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret.len(), 2);
    assert!(ret.get(1).unwrap().1 - ret.first().unwrap().1 >= 1000);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_no_expr_line() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_params_data("hello".into())
        }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_expr_full_line() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1")
                .with_params_data(json!(r#"{{ "hello" }}"#))
        }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_expr_partial_line() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1")
                .with_params_data(json!(r#"{{ "hello" }} world"#))
        }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello world");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_expr_multi_statements() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_params_data(json!(
                r#"{{ let a = "hello";let b = "world"; a + " " + b }}"#
            ))
        }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello world");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_expr_brace_not_in_same_line_not_support() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_params_data(json!(
                r#"{{
                let a = "hello";
                let b = "world";
                a + " " + b
                }}"#
            ))
        }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert_ne!(ret, "hello world");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_multi_expr_str() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", "hello")
            .with_var("b", "world")
            .with_act(Act::irq(|act| {
                act.with_key("act1").with_params_data(json!(
                    r#"
                    let v1 = "{{ a }}";
                    let v2 = "{{ b }}";
                    v1 + " " +  v2
                "#
                ))
            }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert!(ret.contains("\"hello\""));
    assert!(ret.contains("\"world\""));
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_multi_expr_bool() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", true)
            .with_var("b", false)
            .with_act(Act::irq(|act| {
                act.with_key("act1").with_params_data(json!(
                    r#"
                    let v1 = {{ a }};
                    let v2 = {{ b }};
                    v1 && v2
                "#
                ))
            }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert!(ret.contains("let v1 = true;"));
    assert!(ret.contains("let v2 = false;"));
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_multi_expr_others() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!({ "v1": 10 }))
            .with_var("b", json!(["v2"]))
            .with_var("c", json!(null))
            .with_act(Act::irq(|act| {
                act.with_key("act1").with_params_data(json!(
                    r#"
                    let v1 = {{ a }};
                    let v2 = {{ b }};
                    let v3 = {{ c }};
                "#
                ))
            }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();

    assert!(ret.contains(r#"let v1 = {"v1":10};"#));
    assert!(ret.contains(r#"let v2 = ["v2"];"#));
    assert!(ret.contains(r#"let v3 = null;"#));
}
