use crate::{CodePackage, env::Environment};
use acts::{Engine, Principal, Variant, Vars, Workflow};
use serde_json::json;
use serial_test::serial;

// ---- QuickJS environment ----

#[test]
fn js_env_eval_object() {
    let env = Environment::new();
    let result = env
        .eval::<serde_json::Value>(r#"({ "a": 1, "b": "abc" })"#)
        .unwrap();
    assert_eq!(result, json!({ "a": 1, "b": "abc" }));
}

#[test]
fn js_env_eval_array() {
    let env = Environment::new();
    let result = env.eval::<Vec<String>>(r#"["u1", "u2"]"#).unwrap();
    assert_eq!(result, ["u1", "u2"]);
}

#[test]
fn js_env_eval_arithmetic() {
    let env = Environment::new();
    assert_eq!(env.eval::<i64>(r#"2 + 3 * 4"#).unwrap(), 14);
}

#[test]
fn js_env_array_ops() {
    let env = Environment::new();
    assert_eq!(
        env.eval::<Vec<String>>(r#"["a"].union(["b"])"#).unwrap(),
        ["a", "b"]
    );
    assert_eq!(
        env.eval::<Vec<String>>(r#"["a", "b"].intersection(["b", "c"])"#)
            .unwrap(),
        ["b"]
    );
    assert_eq!(
        env.eval::<Vec<String>>(r#"["a", "b"].difference(["b"])"#)
            .unwrap(),
        ["a"]
    );
}

#[test]
fn js_env_throw_is_error() {
    let env = Environment::new();
    let result = env.eval::<serde_json::Value>(r#"throw new Error("boom")"#);
    assert!(result.is_err());
}

// ---- code package ----

async fn run_code(workflow: &Workflow) -> Vars {
    let engine = Engine::builder()
        .add_package::<CodePackage>()
        .start()
        .await
        .unwrap();

    let sig = engine.signal(Vars::new());
    let done = sig.clone();
    engine.channel().on_complete(move |e| {
        let done = done.clone();
        async move {
            done.update(|d| *d = e.outputs.clone());
            done.close();
        }
    });

    let executor = engine.executor(&Principal::unrestricted());
    executor.model().deploy(workflow, None).await.unwrap();
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    sig.recv().await
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_code_outputs() {
    let workflow = Workflow::new().with_id("code_outputs").with_step(|step| {
        step.with_id("step1")
            .with_expose(Variant::create("my_output", json!(null)))
            .with_uses_code("acts.app.javascript", r#"return { "my_output": "abc" };"#)
    });

    let outputs = run_code(&workflow).await;
    assert_eq!(outputs.get::<String>("my_output").unwrap(), "abc");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_code_computes_and_returns() {
    let workflow = Workflow::new().with_id("code_compute").with_step(|step| {
        step.with_id("step1").with_uses_code(
            "acts.app.javascript",
            r#"return { sum: 2 + 3, msg: "hi".toUpperCase() };"#,
        )
    });

    let outputs = run_code(&workflow).await;
    assert_eq!(outputs.get::<i32>("sum").unwrap(), 5);
    assert_eq!(outputs.get::<String>("msg").unwrap(), "HI");
}

/// Task data reaches the JavaScript through `${{ }}` expressions, which the
/// engine evaluates before the script runs.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_code_reads_injected_value() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_var("value", 21)
        .with_step(|step| {
            step.with_id("step1").with_uses_code(
                "acts.app.javascript",
                r#"return { doubled: ${{ value }} * 2 };"#,
            )
        });

    let outputs = run_code(&workflow).await;
    assert_eq!(outputs.get::<i32>("doubled").unwrap(), 42);
}
