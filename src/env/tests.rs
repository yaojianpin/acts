use crate::{
    env::Enviroment, Act, ActError, Context, Engine, Event, Message, Signal, Vars, Workflow,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[test]
fn env_eval_void() {
    let env = Enviroment::new();

    let script = r#"
    let v = 5;
    console.log(`v=${v}`);
    "#;

    let result = env.eval::<()>(script);
    assert_eq!(result.is_ok(), true);
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

    let result = env.eval::<()>(script);
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
    assert_eq!(result.unwrap(), true);
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
async fn env_console_module() {
    let env = Enviroment::new();
    let script = r#"
    let v = 5;
    console.log(`v=${v}`);
    console.info(`v=${v}`);
    console.wran(`v=${v}`);
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
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    let workflow = Workflow::new()
        .with_input("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
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
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    let workflow = Workflow::new()
        .with_input("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
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
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<()>(r#"$("a", 100)"#).unwrap();
        env.eval::<()>(r#"$("b", 200)"#).unwrap();
        let value = env.eval::<bool>(r#"$("a") < $("b")"#).unwrap();
        assert_eq!(value, true);
    });
}

#[tokio::test]
async fn env_env_get_local() {
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    let workflow = Workflow::new()
        .with_env("a", 10.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
    $env("a")
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i64>(script);
        assert_eq!(result.unwrap(), 10);
    });
}

#[tokio::test]
async fn env_env_get_global() {
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    env.set("a", 10);
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    engine.emitter().on_complete(move |_| s1.close());
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();

    sig.recv().await;
    let task = proc.root().unwrap();
    let script = r#"
    $env("a")
    "#;

    let context = task.create_context();
    Context::scope(context, || {
        let result = env.eval::<i64>(script);
        assert_eq!(result.unwrap(), 10);
    });
}

#[tokio::test]
async fn env_env_set_from_global() {
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    env.set("a", 10);
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    // set the env value only change the proc local env in context
    let script = r#"
    $env("a", 100);
    "#;
    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<()>(script).unwrap();
        assert_eq!(proc.env_local().get::<i64>("a"), Some(100));

        // the global env value is not changed
        assert_eq!(env.get::<i64>("a"), Some(10));
    });
}

#[tokio::test]
async fn env_env_set_both_local_global() {
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    env.set("a", 10);
    let workflow = Workflow::new()
        .with_env("a", 100.into())
        .with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    // set the env value only change the proc local env in context
    let script = r#"
    $env("a", 200);
    "#;
    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<()>(script).unwrap();
        assert_eq!(proc.env_local().get::<i64>("a"), Some(200));

        // the global env value is not changed
        assert_eq!(env.get::<i64>("a"), Some(10));
    });
}

#[tokio::test]
async fn env_env_multi_line() {
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();

    let env = engine.env();
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_complete(move |_| s1.close());
    sig.recv().await;
    let task = proc.root().unwrap();

    let context = task.create_context();
    Context::scope(context, || {
        env.eval::<()>(r#"$env("a", 100)"#).unwrap();
        env.eval::<()>(r#"$env("b", 200)"#).unwrap();
        let value = env.eval::<bool>(r#"$env("a") < $env("b")"#).unwrap();
        assert_eq!(value, true);
    });
}

#[test]
fn env_vars_set_num() {
    let env = Enviroment::new();
    env.set("a", 5);
    assert_eq!(env.get::<u32>("a").unwrap(), 5);
    assert_eq!(env.get::<String>("a"), None);
}

#[test]
fn env_vars_set_str() {
    let env = Enviroment::new();
    env.set("a", "abc");
    assert_eq!(env.get::<String>("a").unwrap(), "abc");
}

#[test]
fn env_vars_set_json() {
    let env = Enviroment::new();
    let json = json!({ "count": 1 });
    env.set("a", json.clone());
    assert_eq!(env.get::<serde_json::Value>("a").unwrap(), json);
}

#[test]
fn env_vars_update() {
    let env = Enviroment::new();
    env.set("a", 1);
    env.set("b", "abc");
    env.update(|data| {
        data.set("a", 2);
        data.set("b", "def");
    });
    assert_eq!(env.get::<i32>("a").unwrap(), 2);
    assert_eq!(env.get::<String>("b").unwrap(), "def");
}

#[tokio::test]
async fn env_act_req() {
    let script = r#"
    let req = { id: "act2"}
    act.req(req);
    "#;
    let ret = run_test(script, |e, s| {
        if e.is_key("act2") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn env_act_msg() {
    let script = r#"
    act.msg({ id: "msg1"});
    "#;
    let ret = run_test(script, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn env_act_chain() {
    let script = r#"
    act.chain({ in: "[ \"u1\", \"u2\" ]", run: [{ msg: { id: "msg1" } }] });
    "#;

    let ret: i32 = run_test(script, |e, s| {
        if e.is_key("msg1") {
            s.update(|data| *data += 1);
        }
        if s.data() == 2 {
            s.close();
        }
    })
    .await;
    assert_eq!(ret, 2);
}

#[tokio::test]
async fn env_act_each() {
    let script = r#"
    act.each({ in: "[ \"u1\", \"u2\" ]", run: [{ msg: { id: "msg1" } }] });
    "#;

    let ret: i32 = run_test(script, |e, s| {
        if e.is_key("msg1") {
            s.update(|data| *data += 1);
        }
        if s.data() == 2 {
            s.close();
        }
    })
    .await;
    assert_eq!(ret, 2);
}

#[tokio::test]
async fn env_act_block() {
    let script = r#"
    act.block({ acts: [{ msg: { id: "msg1" } }] });
    "#;

    let ret = run_test(script, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn env_act_call() {
    let script = r#"
    act.call({ mid: "m1" });
    "#;

    let ret = run_test(script, |e, s| {
        if e.is_key("m1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn env_act_push() {
    let script = r#"
    act.push({ req: { id: "act1" } });
    "#;

    let ret = run_test(script, |e, s| {
        if e.is_key("act1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

async fn run_test<T: Clone + Send + 'static + Default>(
    script: &str,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> T {
    let engine = Engine::new();
    let sig1 = engine.signal(());
    let sig2 = engine.signal(T::default());
    let s1 = sig1.clone();
    let s2 = sig2.clone();

    let m1 = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    engine.manager().deploy(&m1).unwrap();

    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let proc = engine.scher().start(&workflow, &Vars::new()).unwrap();
    engine.emitter().on_message(move |e| {
        // println!("message: {e:?}");
        if e.is_key("act1") {
            s1.close();
        }
    });
    engine.emitter().on_message(move |e| exit_if(e, s2.clone()));

    sig1.recv().await;
    let task = proc.root().unwrap();
    let context = task.create_context();
    context.eval::<()>(&script).unwrap();
    sig2.recv().await
}
