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
    for expr in [r#"throw new Error("boom")"#, "throw 42", r#"throw "boom""#] {
        let result = env.eval::<serde_json::Value>(expr);
        assert!(result.is_err(), "{expr} must fail the expression");
    }
}

/// A modules lock another thread panicked under is reported, not unwrapped:
/// the environment stays a library, not a place a panic escapes from.
#[test]
fn js_env_reports_a_poisoned_modules_lock() {
    let env = Environment::new();
    let modules = env.modules.clone();
    let handle = std::thread::spawn(move || {
        let _guard = modules.write().unwrap();
        panic!("poison the modules lock");
    });
    assert!(handle.join().is_err(), "the helper thread panics");

    let err = env.eval::<serde_json::Value>("1").unwrap_err();
    assert!(err.to_string().contains("poisoned"), "{err}");
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

// ---- the JSON <-> JS value boundary ----

use crate::value::ActJsValue;
use acts::ActPackage;
use rquickjs::{FromJs, IntoJs};

fn roundtrip(value: serde_json::Value) -> serde_json::Value {
    let runtime = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&runtime).unwrap();
    ctx.with(|ctx| {
        let js = ActJsValue::new(value).into_js(&ctx).unwrap();
        serde_json::Value::from(ActJsValue::from_js(&ctx, js).unwrap())
    })
}

#[test]
fn js_value_roundtrips_every_json_kind() {
    for value in [
        json!(null),
        json!(true),
        json!(false),
        json!(0),
        json!(-3),
        json!(3.5),
        json!("abc"),
        json!([1, "a", null, [true]]),
        json!({ "a": { "b": [false, 1.5] } }),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(1.0e300),
    ] {
        assert_eq!(roundtrip(value.clone()), value, "{value}");
    }
}

#[test]
fn js_value_accessors_and_conversions() {
    let value = ActJsValue::new(json!({ "a": 1 }));
    assert_eq!(value.inner(), &json!({ "a": 1 }));
    assert_eq!(value.to::<serde_json::Value>().unwrap(), json!({ "a": 1 }));
    assert!(value.to::<Vec<String>>().is_err());

    let from_vars: ActJsValue = Vars::from(json!({ "x": 2 })).into();
    assert_eq!(serde_json::Value::from(from_vars), json!({ "x": 2 }));

    let from_string: ActJsValue = String::from("hi").into();
    assert_eq!(serde_json::Value::from(from_string), json!("hi"));
}

// ---- the environment's error and conversion edges ----

#[test]
fn js_env_console_logs_without_failing() {
    let env = Environment::new();
    let result = env
        .eval::<serde_json::Value>(
            r#"console.log("a", 1, { b: 2 }); console.info("i"); console.warn("w"); console.error("e"); 7"#,
        )
        .unwrap();
    assert_eq!(result, json!(7));
}

#[test]
fn js_env_reads_undefined_as_null() {
    let env = Environment::new();
    assert_eq!(
        env.eval::<serde_json::Value>("undefined").unwrap(),
        json!(null)
    );
    assert_eq!(
        env.eval::<serde_json::Value>("const a = 1;").unwrap(),
        json!(null)
    );
}

/// Values JSON cannot carry are refused instead of being coerced.
#[test]
fn js_env_refuses_unrepresentable_values() {
    let env = Environment::new();
    for expr in [
        "NaN",
        "Infinity",
        "(() => 1)",
        "Symbol('x')",
        "Promise.resolve(1)",
    ] {
        assert!(
            env.eval::<serde_json::Value>(expr).is_err(),
            "{expr} must not convert"
        );
    }
}

#[test]
fn js_env_converts_bigints_or_refuses_overflow() {
    let env = Environment::new();
    assert_eq!(
        env.eval::<serde_json::Value>("9007199254740993n").unwrap(),
        json!(9_007_199_254_740_993_i64)
    );
    assert_eq!(
        env.eval::<serde_json::Value>("18446744073709551615n")
            .unwrap(),
        json!(u64::MAX)
    );
    assert_eq!(
        env.eval::<serde_json::Value>("-9223372036854775808n")
            .unwrap(),
        json!(i64::MIN)
    );
    for expr in ["2n ** 100n", "-(2n ** 100n)"] {
        assert!(
            env.eval::<serde_json::Value>(expr).is_err(),
            "{expr} is outside the i64/u64 range"
        );
    }
}

#[test]
fn js_env_reads_a_proxy_through_its_traps() {
    let env = Environment::new();
    let result = env
        .eval::<serde_json::Value>(r#"new Proxy({ a: 1 }, {})"#)
        .unwrap();
    assert_eq!(result, json!({ "a": 1 }));
}

#[test]
fn js_env_reports_a_deserialization_mismatch() {
    let env = Environment::new();
    let err = env.eval::<Vec<String>>(r#"({ a: 1 })"#).unwrap_err();
    assert!(err.to_string().contains("invalid type"), "{err}");
}

// ---- the code package's own edges ----

#[test]
fn code_definition_is_a_string_parameter() {
    let meta = CodePackage::definition();
    assert_eq!(meta.id, "acts.app.javascript");
    assert_eq!(meta.schema["type"], "string");
    CodePackage::new(&acts::Config::default()).expect("no configuration is required");
}

async fn run_code_error(workflow: &Workflow) -> Vars {
    let engine = Engine::builder()
        .add_package::<CodePackage>()
        .start()
        .await
        .unwrap();

    let sig = engine.signal(Vars::new());
    let failed = sig.clone();
    engine.channel().on_error(move |e| {
        let failed = failed.clone();
        async move {
            failed.update(|d| *d = e.outputs.clone());
            failed.close();
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
async fn pack_code_non_object_result_has_no_outputs() {
    let workflow = Workflow::new().with_id("code_scalar").with_step(|step| {
        step.with_id("step1")
            .with_uses_code("acts.app.javascript", "return 42;")
    });
    let outputs = run_code(&workflow).await;
    assert!(outputs.get::<i32>("x").is_none(), "{outputs}");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_code_refuses_a_non_string_parameter() {
    let workflow = Workflow::from_yml(
        r#"
name: js bad params
id: code_bad_params
ver: "0.1.0"
steps:
  - id: step1
    uses: acts.app.javascript
    params: 42
"#,
    )
    .unwrap();

    let outputs = run_code_error(&workflow).await;
    assert!(
        outputs.to_string().contains("requires a string parameter")
            || outputs.to_string().contains("validation error"),
        "{outputs}"
    );
}

#[test]
fn js_env_integral_floats_within_the_safe_range_are_json_integers() {
    let env = Environment::new();
    assert_eq!(
        env.eval::<serde_json::Value>("3e10").unwrap(),
        json!(30_000_000_000_i64)
    );
}

#[test]
fn js_value_sends_wide_integers_as_doubles_while_they_are_exact() {
    assert_eq!(
        roundtrip(json!(3_000_000_000_i64)),
        json!(3_000_000_000_i64)
    );
}

/// A proxy whose `get` trap throws is read like an object: the failing
/// property is simply not part of the value.
#[test]
fn js_env_reads_a_proxy_whose_get_trap_throws() {
    let env = Environment::new();
    let result = env
        .eval::<serde_json::Value>(r#"new Proxy({ a: 1 }, { get() { throw new Error("no") } })"#)
        .unwrap();
    assert_eq!(result, json!({}));
}

/// A caught exception is a JS value of its own: converting it re-throws it
/// instead of turning it into JSON.
#[test]
fn js_value_rethrows_a_caught_exception() {
    let runtime = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&runtime).unwrap();
    ctx.with(|ctx| {
        let _ = ctx.eval::<rquickjs::Value, _>(r#"throw new Error("boom")"#);
        let caught = ctx.catch();
        assert!(
            ActJsValue::from_js(&ctx, caught).is_err(),
            "an exception has no JSON representation"
        );
    });
}

#[test]
fn environment_has_a_debug_representation_and_a_default() {
    let env = Environment::default();
    assert!(format!("{env:?}").contains("Environment"));
}

#[test]
fn js_env_stops_a_script_that_outlives_the_timeout() {
    let env = Environment::new();
    let start = std::time::Instant::now();
    let err = env
        .eval::<serde_json::Value>("while (true) {}")
        .unwrap_err();
    assert!(err.to_string().contains("timeout"), "{err}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(30),
        "the interrupt handler must end the loop"
    );
}
