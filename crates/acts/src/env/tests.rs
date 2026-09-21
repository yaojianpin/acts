use std::sync::Arc;
use std::time::Duration;

use crate::{
    ActUserVar, Context, Engine, MessageState, Vars, Workflow, env::Environment,
    event::EventAction, utils::consts, utils::test::USES_IRQ,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serial_test::serial;

/// Start an engine, run a workflow to completion, and hand back the engine and
/// the finished process. The workflow must finish on its own (no IRQ wait).
async fn start_completed(
    workflow: &Workflow,
    vars: Vars,
) -> (Engine, Arc<crate::scheduler::Process>) {
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    let proc = engine.runtime().start(workflow, vars).await.unwrap();
    sig.recv().await;
    (engine, proc)
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

#[test]
fn env_eval_array() {
    let env = Environment::new();
    assert_eq!(
        env.eval::<Vec<String>>("['u1', 'u2']").unwrap(),
        ["u1", "u2"]
    );
}

#[test]
fn env_eval_object() {
    #[derive(Debug, Deserialize, Serialize, PartialEq, Clone)]
    struct Obj {
        a: i32,
        b: String,
    }

    let env = Environment::new();
    let result = env.eval::<Obj>("{'a': 1, 'b': 'abc'}").unwrap();
    assert_eq!(
        result,
        Obj {
            a: 1,
            b: "abc".to_string()
        }
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
    assert!(env.eval::<serde_json::Value>("this is not cel").is_err());
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
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
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
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        assert_eq!(env.eval::<i64>("a").unwrap(), 10);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_get_var_not_exists() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        assert!(env.eval::<serde_json::Value>("not_exists").is_err());
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_get_fn_not_exists() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        assert_eq!(
            env.eval::<serde_json::Value>("$get('not_exists')").unwrap(),
            serde_json::Value::Null
        );
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_set() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_var("a", 10)
        .with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        env.eval::<()>("$set('a', 100)").unwrap();
        assert_eq!(proc.data().get::<i64>("a"), Some(100));
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_task_multi_eval() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        env.eval::<()>("$set('a', 100)").unwrap();
        env.eval::<()>("$set('b', 200)").unwrap();
        assert!(env.eval::<bool>("a < b").unwrap());
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_env_get_local() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_env("a", 10)
        .with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        assert_eq!(env.eval::<i64>("$env.a").unwrap(), 10);
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
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    context.set_env(crate::utils::consts::PROC_OWNER, "forged");

    Context::scope(&context, || {
        let read =
            env.eval::<serde_json::Value>(&format!("$env.{}", crate::utils::consts::PROC_OWNER));
        assert!(read.is_err(), "private key must not be readable");
    });
}

/// The directory a process runs in has a readable name of its own —
/// `$env.WORK_DIR` — answered from the process rather than stored.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_work_dir_names_the_process_directory() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    let dir = std::env::temp_dir().join(format!("acts_workdir_{}", crate::utils::longid()));
    proc.set_workdir(&dir);

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
    assert_eq!(proc.workdir(), Some(dir));
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
    let proc = engine
        .runtime()
        .start(
            &workflow,
            Vars::new().with("test", Vars::new().with("var1", 10)),
        )
        .await
        .unwrap();
    sig.recv().await;

    let context = proc.root().unwrap().create_context();
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
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        assert_eq!(env.eval::<i32>("test.var1").unwrap(), 5);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_user_var_secrets_get() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_engine, proc) = start_completed(
        &workflow,
        Vars::new().with("secrets", Vars::new().with("TOKEN", "my_token")),
    )
    .await;

    let context = proc.root().unwrap().create_context();
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
    let (_e1, p1) = start_completed(&w1, Vars::new()).await;
    let c1 = p1.root().unwrap().create_context();
    Context::scope(&c1, || assert_eq!(env.eval::<i64>("a").unwrap(), 1));

    let w2 = Workflow::new().with_step(|step| step.with_id("step1"));
    let (_e2, p2) = start_completed(&w2, Vars::new()).await;
    let c2 = p2.root().unwrap().create_context();
    Context::scope(&c2, || assert!(env.eval::<i64>("a").is_err()));
}

// ---- step data access ----

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_step_get_data_by_id() {
    let env = Environment::new();
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_var("a", 10))
        .with_step(|step| step.with_id("step2").with_var("b", "abc"));
    let (_engine, proc) = start_completed(&workflow, Vars::new()).await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        assert_eq!(env.eval::<i32>("step1.a").unwrap(), 10);
        assert_eq!(env.eval::<String>("step2.b").unwrap(), "abc");
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_step_get_data() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("b", "abc")
            .with_uses(USES_IRQ, Vars::new().with("key", "test"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_message(move |e| {
        let done = done.clone();
        async move {
            if e.is_irq() {
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        let result = env.eval::<Vars>("$step_data('step1')").unwrap();
        assert_eq!(result.get::<String>("b").unwrap(), "abc");
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_step_get_inputs() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", 10)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_message(move |e| {
        let done = done.clone();
        async move {
            if e.is_irq() {
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;

    let context = proc.root().unwrap().create_context();
    Context::scope(&context, || {
        let result = env.eval::<Vars>("$step_inputs('step1')").unwrap();
        assert_eq!(result.get::<i32>("a").unwrap(), 10);
    });
}

// ---- act data access ----

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_act_get_inputs() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(10))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_message(move |e| {
        let done = done.clone();
        async move {
            if e.is_irq() {
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();

    let context = task.create_context();
    Context::scope(&context, || {
        let result = env.eval::<Vars>("$inputs()").unwrap();
        assert_eq!(result.get::<i32>("a").unwrap(), 10);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_act_get_data() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_message(move |e| {
        let done = done.clone();
        async move {
            if e.is_irq() {
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();

    let context = task.create_context();
    Context::scope(&context, || {
        env.eval::<()>("$set('my_value', 20)").unwrap();
        let result = env.eval::<Vars>("$data()").unwrap();
        assert_eq!(result.get::<i32>("my_value").unwrap(), 20);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_act_cost_get() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_message(move |e| {
        let done = done.clone();
        async move {
            if e.is_irq() {
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();

    let context = task.create_context();
    Context::scope(&context, || {
        assert!(env.eval::<i64>("$cost()").unwrap() > 0);
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_act_cost_in_get() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_message(move |e| {
        let done = done.clone();
        async move {
            if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
                && e.is_state(MessageState::Created)
            {
                tokio::time::sleep(Duration::from_secs(2)).await;
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();

    let context = task.create_context();
    Context::scope(&context, || {
        assert!(env.eval::<bool>("$cost_in('1s')").unwrap());
    });
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn env_act_ecode_get() {
    let env = Environment::new();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    let runtime = engine.runtime();
    engine.channel().on_message(move |e| {
        let runtime = runtime.clone();
        let done = done.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                runtime
                    .do_action2(
                        &e.pid,
                        &e.tid,
                        EventAction::Error,
                        Vars::new().with(consts::ACT_ERR_CODE, "err1"),
                    )
                    .await
                    .unwrap();
                done.close()
            }
        }
    });
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();

    let context = task.create_context();
    Context::scope(&context, || {
        assert_eq!(env.eval::<String>("$ecode()").unwrap(), "err1");
    });
}
