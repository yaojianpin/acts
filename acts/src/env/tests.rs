use crate::{Act, ActError, ActUserVar, Context, Engine, Vars, Workflow, env::Enviroment};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[test]
fn env_eval_empty() {
    let env = Enviroment::new();
    let result = env.eval::<()>("");
    assert!(result.is_ok());
}

#[test]
fn env_eval_void() {
    let env = Enviroment::new();

    let script = r#"
        let v = 5;
        console.log(`v=${v}`);
    "#;

    let result = env.eval::<()>(script);
    assert!(result.is_ok());
}

#[test]
fn env_eval_number() {
    let env = Enviroment::new();
    let script = r#"
        let v = 5;
        v
    "#;

    let result = env.eval::<i64>(script);
    assert_eq!(result.unwrap(), 5);
}

#[test]
fn env_eval_throw_error() {
    let env = Enviroment::new();
    let script = r#"
        throw new Error("err1");
    "#;

    let result = env.eval::<serde_json::Value>(script);
    assert_eq!(
        result.err().unwrap(),
        ActError::Exception {
            ecode: "".to_string(),
            message: "err1".to_string()
        }
    );
}

#[test]
fn env_eval_expr() {
    let env = Enviroment::new();

    let script = r#"
        let ret =  10;
        ret > 0
    "#;
    let result = env.eval::<bool>(script);
    assert!(result.unwrap());
}

#[test]
fn env_eval_array() {
    let env = Enviroment::new();

    let script = r#"
        ["u1", "u2"]
    "#;

    let result = env.eval::<Vec<String>>(script);
    assert_eq!(result.unwrap(), ["u1", "u2"]);
}

#[test]
fn env_eval_object() {
    let env = Enviroment::new();

    let script = r#"
        let ret =  { "a": 1, "b": "abc" };
        ret
    "#;

    #[derive(Debug, Deserialize, Serialize, PartialEq, Clone)]
    struct Obj {
        a: i32,
        b: String,
    }
    let result = env.eval::<Obj>(script);
    assert_eq!(
        result.unwrap(),
        Obj {
            a: 1,
            b: "abc".to_string()
        }
    );
}

#[tokio::test]
async fn env_eval_sys_env() {
    unsafe {
        std::env::set_var("TOKEN", "abc");
    }
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        $env.TOKEN
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<String>(script);
        assert_eq!(result.unwrap(), "abc");
    });
}

#[tokio::test]
async fn env_eval_null() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        $env.TOKEN2
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<serde_json::Value>(script);
        assert_eq!(result.unwrap(), serde_json::Value::Null);
    });
}

#[tokio::test]
async fn env_console_module() {
    let env = Enviroment::new();
    let script = r#"
        let v = 5;
        console.log(`v=${v}`);
        console.info(`v=${v}`);
        console.warn(`v=${v}`);
        console.error(`v=${v}`);
    "#;
    let result = env.eval::<()>(script);
    assert!(result.is_ok());
}

#[test]
fn env_collection_union() {
    let env = Enviroment::new();
    let script = r#"
        let a = ["a"];
        let b = ["b"];
        a.union(b)
    "#;

    let result = env.eval::<Vec<String>>(script).unwrap();
    assert_eq!(result, ["a", "b"]);
}

#[test]
fn env_collection_intersect() {
    let env = Enviroment::new();
    let script = r#"
        let a = ["a", "b"];
        let b = ["b", "c"];
        a.intersection(b)
    "#;

    let result = env.eval::<Vec<String>>(script).unwrap();
    assert_eq!(result, ["b"]);
}

#[test]
fn env_collection_difference() {
    let env = Enviroment::new();
    let script = r#"
        let a = ["a", "b"];
        let b = ["b"];
        a.difference(b)
    "#;

    let result = env.eval::<Vec<String>>(script).unwrap();
    assert_eq!(result, ["a"]);
}

#[tokio::test]
async fn env_task_get() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new()
        .with_input("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        $("a")
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i64>(script);
        assert_eq!(result.unwrap(), 10);
    });
}

#[tokio::test]
async fn env_task_set() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new()
        .with_input("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        $("a", 100);
    "#;
    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<()>(script).unwrap();
        assert_eq!(proc.data().get::<i64>("a"), Some(100));
    });
}

#[tokio::test]
async fn env_task_multi_line() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<()>(r#"$("a", 100)"#).unwrap();
        env.eval::<()>(r#"$("b", 200)"#).unwrap();
        let value = env.eval::<bool>(r#"$("a") < $("b")"#).unwrap();
        assert!(value);
    });
}

#[tokio::test]
async fn env_env_get_local() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new()
        .with_env("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
    $env.a
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i64>(script);
        assert_eq!(result.unwrap(), 10);
    });
}

#[tokio::test]
async fn env_env_set_proc_env() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new()
        .with_env("a", 100.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    // set the env value only change the process local env in context
    let script = r#"
    $env.a = 200;
    "#;
    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<serde_json::Value>(script).unwrap();
        assert_eq!(proc.env().get::<i64>("a"), Some(200));
    });
}

#[tokio::test]
async fn env_env_multi_line() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<serde_json::Value>(r#"$env.a = 100"#).unwrap();
        env.eval::<serde_json::Value>(r#"$env.b = 200"#).unwrap();
        let value = env.eval::<bool>(r#"$env.a < $env.b"#).unwrap();
        assert!(value);
    });
}

#[tokio::test]
async fn env_vars_set_num() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let workflow = Workflow::new()
        .with_env("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        assert_eq!(proc.env().get::<i64>("a"), Some(10));
    });
}

#[tokio::test]
async fn env_vars_set_str() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let workflow = Workflow::new()
        .with_env("a", "abc".into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        assert_eq!(proc.env().get::<String>("a"), Some("abc".to_string()));
    });
}

#[tokio::test]
async fn env_vars_set_json() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let workflow = Workflow::new()
        .with_env("a", json!({ "count": 1 }))
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        assert_eq!(
            proc.env().get::<serde_json::Value>("a"),
            Some(json!({ "count": 1 }))
        );
    });
}

#[tokio::test]
async fn env_vars_update() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let workflow = Workflow::new()
        .with_env("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    context.set_env("a", 100);
    Context::scope(context, || {
        assert_eq!(proc.env().get::<i32>("a"), Some(100));
    });
}

#[tokio::test]
async fn env_step_get_data_by_id() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_input("a", 10.into()))
        .with_step(|step| step.with_id("step2").with_input("b", "abc".into()));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        step1.a
    "#;

    proc.print();
    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i32>(script);
        assert_eq!(result.unwrap(), 10);
    });

    let script = r#"
        step2.b
    "#;
    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<String>(script);
        assert_eq!(result.unwrap(), "abc");
    });
}

#[tokio::test]
async fn env_step_get_data_null() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_input("a", 10.into()));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        step1.not_exists
    "#;

    proc.print();
    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<serde_json::Value>(script);
        assert_eq!(result.unwrap(), serde_json::Value::Null);
    });
}

#[tokio::test]
async fn env_step_set_data_err_with_completed_state() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_input("a", 10.into()));
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    engine.channel().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        step1.a = 100;
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<serde_json::Value>(script);
        proc.print();
        assert!(result.is_err());
    });
}

#[tokio::test]
async fn env_step_set_data_ok_with_running_state() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", 10.into())
            .with_act(Act::irq(|act| act.with_key("test")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        step1.a = 100;
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<serde_json::Value>(script).unwrap();
        proc.print();
        assert_eq!(
            proc.task_by_nid("step1")
                .last()
                .unwrap()
                .data()
                .get::<i32>("a")
                .unwrap(),
            100
        );
    });
}

#[tokio::test]
async fn env_step_get_data() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", 10.into())
            .with_act(Act::irq(|act| act.with_key("test")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        step1.b = "abc";
        step1.data()
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<Vars>(script).unwrap();
        proc.print();
        assert_eq!(result.get::<String>("b").unwrap(), "abc");
    });
}

#[tokio::test]
async fn env_step_get_inputs() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", 10.into())
            .with_act(Act::irq(|act| act.with_key("test")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
        step1.inputs()
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<Vars>(script).unwrap();
        proc.print();
        assert_eq!(result.get::<i32>("a").unwrap(), 10);
    });
}

#[tokio::test]
async fn env_act_get_inputs() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("test").with_id("act1").with_input("a", 10)
        }))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    sig.recv().await;
    let task = proc.task_by_nid("act1").last().cloned().unwrap();
    let script = r#"
        $act.inputs()
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<Vars>(script).unwrap();
        proc.print();
        assert_eq!(result.get::<i32>("a").unwrap(), 10);
    });
}

#[tokio::test]
async fn env_act_get_data() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("test").with_id("act1")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    sig.recv().await;
    let task = proc.task_by_nid("act1").last().cloned().unwrap();
    let script = r#"
        $act.set("my_value", 20)
        $act.data()
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<Vars>(script).unwrap();
        proc.print();
        assert_eq!(result.get::<i32>("my_value").unwrap(), 20);
    });
}

#[tokio::test]
async fn env_user_var_get_from_context() {
    #[derive(Clone)]
    struct MyVarPlugin;

    impl ActUserVar for MyVarPlugin {
        fn name(&self) -> String {
            "test".to_string()
        }
    }

    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    engine.extender().register_var(&MyVarPlugin);

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("test").with_id("act1")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine
        .runtime()
        .start(
            &workflow,
            &Vars::new().with("test", Vars::new().with("var1", 10)),
        )
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_nid("act1").last().cloned().unwrap();
    let script = r#"
        test.var1
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i32>(script).unwrap();
        proc.print();
        assert_eq!(result, 10);
    });
}

#[tokio::test]
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

    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    engine.extender().register_var(&MyVarPlugin);

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("test").with_id("act1")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine.runtime().start(&workflow, &Vars::new()).unwrap();
    sig.recv().await;
    let task = proc.task_by_nid("act1").last().cloned().unwrap();
    let script = r#"
        test.var1
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i32>(script).unwrap();
        proc.print();
        assert_eq!(result, 5);
    });
}

#[tokio::test]
async fn env_user_var_secrets_get() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("test").with_id("act1")))
    });
    engine.channel().on_message(move |e| {
        if e.is_irq() {
            s1.close()
        }
    });
    let proc = engine
        .runtime()
        .start(
            &workflow,
            &Vars::new().with("secrets", Vars::new().with("TOKEN", "my_token")),
        )
        .unwrap();
    sig.recv().await;
    let task = proc.task_by_nid("act1").last().cloned().unwrap();
    let script = r#"
        secrets.TOKEN
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<String>(script).unwrap();
        proc.print();
        assert_eq!(result, "my_token");
    });
}
