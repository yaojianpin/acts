use crate::event::EventAction;
use crate::{
    Act, MessageState, Vars, Workflow,
    utils::{self, test::create_proc_signal},
};
use serde_json::json;

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

#[tokio::test]
async fn sch_act_params_no_expr_line() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_params_data("hello".into())
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello");
}

#[tokio::test]
async fn sch_act_params_expr_full_line() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1")
                .with_params_data(json!(r#"{{ "hello" }}"#))
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello");
}

#[tokio::test]
async fn sch_act_params_expr_partial_line() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1")
                .with_params_data(json!(r#"{{ "hello" }} world"#))
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello world");
}

#[tokio::test]
async fn sch_act_params_expr_multi_statements() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_params_data(json!(
                r#"{{ let a = "hello";let b = "world"; a + " " + b }}"#
            ))
        }))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret, "hello world");
}

#[tokio::test]
async fn sch_act_params_expr_brace_not_in_same_line_not_support() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_ne!(ret, "hello world");
}

#[tokio::test]
async fn sch_act_params_multi_expr_str() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", "hello".into())
            .with_input("b", "world".into())
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
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert!(ret.contains("\"hello\""));
    assert!(ret.contains("\"world\""));
}

#[tokio::test]
async fn sch_act_params_multi_expr_bool() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", true.into())
            .with_input("b", false.into())
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
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert!(ret.contains("let v1 = true;"));
    assert!(ret.contains("let v2 = false;"));
}

#[tokio::test]
async fn sch_act_params_multi_expr_others() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!({ "v1": 10 }))
            .with_input("b", json!(["v2"]))
            .with_input("c", json!(null))
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
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<String>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            let params = e.inputs.get::<String>("params").unwrap();
            rx.send(params);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert!(ret.contains(r#"let v1 = {"v1":10};"#));
    assert!(ret.contains(r#"let v2 = ["v2"];"#));
    assert!(ret.contains(r#"let v3 = null;"#));
}
