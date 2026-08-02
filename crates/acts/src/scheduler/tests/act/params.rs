use crate::{
    MessageState, Vars, Workflow,
    utils::{
        self,
        test::{USES_IRQ, auto_complete, create_proc},
    },
};
use serde_json::json;

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_no_expr_line() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_IRQ,
            Vars::new().with("key", "act1").with("data", "hello"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
        step.with_id("step1").with_uses(
            USES_IRQ,
            Vars::new()
                .with("key", "act1")
                .with("data", json!(r#"${{ "hello" }}"#)),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
        step.with_id("step1").with_uses(
            USES_IRQ,
            Vars::new()
                .with("key", "act1")
                .with("data", r#"${{ "hello" }} world"#),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
        step.with_id("step1").with_uses(
            USES_IRQ,
            Vars::new().with("key", "act1").with(
                "data",
                json!(r#"${{ let a = "hello";let b = "world"; a + " " + b }}"#),
            ),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
async fn sch_act_params_expr_multi_line() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_IRQ,
            Vars::new().with("key", "act1").with(
                "data",
                json!(
                    r#"${{
                let a = "hello";
                let b = "world";
                a + " " + b
                }}"#
                ),
            ),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
            .with_uses(
                USES_IRQ,
                Vars::new().with("key", "act1").with(
                    "data",
                    json!(
                        r#"
                    let v1 = "${{ a }}";
                    let v2 = "${{ b }}";
                    v1 + " " +  v2
                "#
                    ),
                ),
            )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
            .with_uses(
                USES_IRQ,
                Vars::new().with("key", "act1").with(
                    "data",
                    json!(
                        r#"
                    let v1 = ${{ a }};
                    let v2 = ${{ b }};
                    v1 && v2
                "#
                    ),
                ),
            )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
            .with_uses(
                USES_IRQ,
                Vars::new().with("key", "act1").with(
                    "data",
                    json!(
                        r#"
                    let v1 = ${{ a }};
                    let v2 = ${{ b }};
                    let v3 = ${{ c }};
                "#
                    ),
                ),
            )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(String::default()).double();
    auto_complete(&engine, &tx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let params = e.params().unwrap().get::<String>("data").unwrap();
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
