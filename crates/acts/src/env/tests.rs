use crate::{ActUserVar, Context, Engine, Vars, Workflow, env::Environment, utils::consts};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serial_test::serial;

/// Build the root context of a finished run from its projected rows.
async fn completed_context(engine: &Engine, pid: &str) -> Context {
    let runtime = engine.runtime();
    let read = runtime
        .read_proc(pid)
        .await
        .unwrap()
        .expect("the finished run has rows");
    crate::scheduler::read_context(&runtime, &read).unwrap()
}

/// Start an engine, run a workflow to completion, and hand back the engine and
/// the finished run's root context. The workflow must finish on its own (no
/// IRQ wait).
async fn start_completed(workflow: &Workflow, vars: Vars) -> (Engine, Context) {
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    let pid = engine.runtime().start(workflow, vars).await.unwrap();
    sig.recv().await;
    let context = completed_context(&engine, &pid).await;
    (engine, context)
}

// ---- expression basics ----

#[test]
fn env_eval_number() {
    let env = Environment::new();
    assert_eq!(env.eval::<i64>("5").unwrap(), 5);
}

#[test]
fn env_eval_arithmetic() {
    let env = Environment::new();
    assert_eq!(env.eval::<i64>("2 + 3 * 4").unwrap(), 14);
    assert_eq!(env.eval::<i64>("(2 + 3) * 4").unwrap(), 20);
}

#[test]
fn env_eval_bool() {
    let env = Environment::new();
    assert!(env.eval::<bool>("10 > 0").unwrap());
    assert!(!env.eval::<bool>("10 < 0").unwrap());
    assert!(env.eval::<bool>("'a' == 'a' && 1 != 2").unwrap());
}

#[test]
fn env_eval_string() {
    let env = Environment::new();
    assert_eq!(env.eval::<String>("'hello'").unwrap(), "hello");
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_eval_array() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_var("list", json!(["u1", "u2"]))
        .with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<Vec<String>>("list").unwrap(), ["u1", "u2"]);
        assert_eq!(env.eval::<String>("list[1]").unwrap(), "u2");
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_eval_object() {
    #[derive(Debug, Deserialize, Serialize, PartialEq, Clone)]
    struct Obj {
        a: i32,
        b: String,
    }

    let env = Environment::new();
    let workflow = Workflow::new()
        .with_var("obj", json!({ "a": 1, "b": "abc" }))
        .with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        let result = env.eval::<Obj>("obj").unwrap();
        assert_eq!(
            result,
            Obj {
                a: 1,
                b: "abc".to_string()
            }
        );
        assert_eq!(env.eval::<String>("obj.b").unwrap(), "abc");
    });
}

/// Collections are injected, not written into an expression: the evaluator is
/// an expression evaluator, and a literal would be a second, differently
/// specified way to spell a value the workflow already holds.
#[test]
fn env_eval_collection_literals_are_rejected() {
    let env = Environment::new();

    for source in ["['u1', 'u2']", "{'a': 1}"] {
        let err = env.eval::<serde_json::Value>(source).unwrap_err();
        assert!(err.to_string().contains("inject"), "{source}: {err}");
    }
}

/// A built-in method inside arithmetic, through the engine's own path (the
/// crate's tests cover the methods themselves; this pins them where a workflow
/// uses them, past the JSON bridge).
#[test]
fn env_eval_method_with_arithmetic() {
    let env = Environment::new();

    assert_eq!(
        env.eval::<i64>(r#"("item".length() + 3) * 1000"#).unwrap(),
        7000
    );
    assert!(
        env.eval::<bool>(r#""item".contains("te") && "item".startsWith("it")"#)
            .unwrap()
    );
}

#[test]
fn env_eval_null() {
    let env = Environment::new();
    assert_eq!(env.eval::<serde_json::Value>("null").unwrap(), json!(null));
}

#[test]
fn env_eval_parse_error() {
    let env = Environment::new();
    assert!(
        env.eval::<serde_json::Value>("this is not an expression")
            .is_err()
    );
}

#[test]
fn env_eval_undeclared_var_is_error() {
    let env = Environment::new();
    assert!(env.eval::<serde_json::Value>("not_exists").is_err());
}

#[test]
fn env_eval_division_by_zero_is_error() {
    let env = Environment::new();
    assert!(env.eval::<serde_json::Value>("1 / 0").is_err());
}

#[test]
fn env_eval_large_numbers_round_trip_exactly() {
    #[derive(Clone)]
    struct BigNumberVar;

    impl ActUserVar for BigNumberVar {
        fn name(&self) -> String {
            "bignumbers".to_string()
        }

        fn default_data(&self) -> Option<Vars> {
            Some(
                Vars::new()
                    .with("over_i32", 3_000_000_000i64)
                    .with("millis", 1_757_318_400_000i64)
                    .with("snowflake", 1_234_567_890_123_456_789i64)
                    .with("min", i64::MIN)
                    .with("over_i64", u64::MAX),
            )
        }
    }

    let env = Environment::new();
    env.register_var(&BigNumberVar);

    // Above i32, inside the i64 range: preserved exactly.
    assert_eq!(
        env.eval::<i64>("bignumbers.over_i32").unwrap(),
        3_000_000_000
    );
    assert_eq!(
        env.eval::<i64>("bignumbers.millis").unwrap(),
        1_757_318_400_000
    );
    assert_eq!(
        env.eval::<i64>("bignumbers.snowflake").unwrap(),
        1_234_567_890_123_456_789
    );
    assert_eq!(env.eval::<i64>("bignumbers.min").unwrap(), i64::MIN);
    assert_eq!(env.eval::<u64>("bignumbers.over_i64").unwrap(), u64::MAX);

    // Arithmetic on the injected value stays exact.
    assert_eq!(
        env.eval::<i64>("bignumbers.snowflake - 1").unwrap(),
        1_234_567_890_123_456_788
    );
}

// ---- engine-context expressions ----

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_eval_sys_env() {
    unsafe {
        std::env::set_var("TOKEN", "abc");
    }
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<String>("$env.TOKEN").unwrap(), "abc");
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_get_value() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_var("a", 10)
        .with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<i64>("a").unwrap(), 10);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_get_var_not_exists() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert!(env.eval::<serde_json::Value>("not_exists").is_err());
    });
}

/// A run with no timeout carries no `__cost` datum, so `$cost_in` reads the
/// acting task's elapsed cost instead of a frozen timeout marker.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_cost_without_a_timeout_reads_the_elapsed_cost() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert!(env.eval::<bool>("$cost_in('0s')").unwrap());
        assert!(!env.eval::<bool>("$cost_in('100000d')").unwrap());
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_get_fn_not_exists() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert_eq!(
            env.eval::<serde_json::Value>("$get('not_exists')").unwrap(),
            serde_json::Value::Null
        );
    });
}

/// A name the expression builds itself is the one thing `$get` is for:
/// everything else is read by its own name — `a > b`, `step1.total`,
/// `secrets.TOKEN` — which is what an expression is supposed to look like.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_get_a_name_the_expression_builds() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_var("prefix", "var")
        .with_var("var_a", 7)
        .with_var("var_b", 9)
        .with_step(|step| step.with_id("step1").with_var("total", 8));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        // Two vars by name: the condition a workflow is written as.
        assert!(env.eval::<bool>("var_b > var_a").unwrap());
        assert_eq!(env.eval::<i64>("var_a + var_b").unwrap(), 16);
        // The same comparison, with the name built at runtime.
        assert_eq!(env.eval::<i64>("$get(prefix + '_a')").unwrap(), 7);
        assert!(env.eval::<bool>("$get(prefix + '_b') > var_a").unwrap());
    });
}

/// Expressions are read-only: the setters are gone and `$env` cannot be
/// assigned to. Variables change only through a package's returned outputs.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_expressions_are_read_only() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_var("a", 10)
        .with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        for source in [
            "$set('a', 100)",
            "$set_process_var('a', 100)",
            "$set_step_value('step1', 'a', 100)",
            "$env.TEST1 = 'value'",
        ] {
            assert!(
                env.eval::<serde_json::Value>(source).is_err(),
                "{source} must be rejected"
            );
        }
        assert_eq!(context.task().data().get::<i64>("a"), Some(10));
    });
}

/// The engine's private env keys (the process owner credential and its
/// workdir) are not workflow state: `$env` must not read them, or a model
/// could widen its own scope authority.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_private_keys_are_engine_only() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    context
        .proc
        .with_env_mut(|env| env.set(crate::utils::consts::PROC_OWNER, "forged"));

    Context::scope(&context, || {
        let read =
            env.eval::<serde_json::Value>(&format!("$env.{}", crate::utils::consts::PROC_OWNER));
        assert!(read.is_err(), "private key must not be readable");
    });
}

/// A workflow's declared `env` is readable as `$env.X` and cannot be changed
/// at run time.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_workflow_env_is_read_only() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_env("TEST1", "value")
        .with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<String>("$env.TEST1").unwrap(), "value");
        assert!(
            env.eval::<serde_json::Value>("$env.TEST1 = 'other'")
                .is_err()
        );
    });
}

/// The directory a process runs in has a readable name of its own —
/// `$env.WORK_DIR` — answered from the process rather than stored.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_work_dir_names_the_process_directory() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(&workflow, Vars::new()).await;
    let dir = std::env::temp_dir().join(format!("acts_workdir_{}", crate::utils::longid()));
    context
        .proc
        .with_env_mut(|env| env.set(consts::PROC_WORKDIR, &dir));

    Context::scope(&context, || {
        assert_eq!(
            env.eval::<String>("$env.WORK_DIR").unwrap(),
            dir.display().to_string()
        );
        assert_eq!(
            context.get_env::<std::path::PathBuf>(consts::ENV_WORK_DIR),
            Some(dir.clone())
        );
    });
    assert_eq!(context.proc.workdir(), Some(dir));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_user_var_get_from_context() {
    #[derive(Clone)]
    struct MyVarPlugin;

    impl ActUserVar for MyVarPlugin {
        fn name(&self) -> String {
            "test".to_string()
        }
    }

    let engine = Engine::builder().start().await.unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .ext()
        .register_var(&MyVarPlugin)
        .unwrap();
    let env = engine.runtime().env().clone();

    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });

    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let pid = engine
        .runtime()
        .start(
            &workflow,
            Vars::new().with("test", Vars::new().with("var1", 10)),
        )
        .await
        .unwrap();
    sig.recv().await;
    let context = completed_context(&engine, &pid).await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<i32>("test.var1").unwrap(), 10);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_user_var_get_default() {
    #[derive(Clone)]
    struct MyVarPlugin;

    impl ActUserVar for MyVarPlugin {
        fn name(&self) -> String {
            "test".to_string()
        }

        fn default_data(&self) -> Option<Vars> {
            Some(Vars::new().with("var1", 5))
        }
    }

    let engine = Engine::builder().start().await.unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .ext()
        .register_var(&MyVarPlugin)
        .unwrap();
    let env = engine.runtime().env().clone();

    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });

    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let pid = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let context = completed_context(&engine, &pid).await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<i32>("test.var1").unwrap(), 5);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_user_var_secrets_get() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, context) = start_completed(
        &workflow,
        Vars::new().with("secrets", Vars::new().with("TOKEN", "my_token")),
    )
    .await;
    Context::scope(&context, || {
        assert_eq!(env.eval::<String>("secrets.TOKEN").unwrap(), "my_token");
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_user_var_os_get() {
    let env = Environment::new();
    let result = env.eval::<String>("$os").unwrap();
    assert!(["linux", "windows", "macos"].contains(&result.as_str()));
}

/// Each evaluation derives a fresh CEL scope from the shared function root:
/// variables injected for one task are not visible to a later evaluation that
/// runs against a different task.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_eval_scopes_do_not_leak_vars() {
    let env = Environment::new();

    let w1 = Workflow::new()
        .with_var("a", 1)
        .with_step(|step| step.with_id("step1"));
    let (_e1, c1) = start_completed(&w1, Vars::new()).await;
    Context::scope(&c1, || assert_eq!(env.eval::<i64>("a").unwrap(), 1));

    let w2 = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_e2, c2) = start_completed(&w2, Vars::new()).await;
    Context::scope(&c2, || assert!(env.eval::<i64>("a").is_err()));
}
