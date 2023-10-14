use crate::{
    env::Enviroment,
    sch::{NodeTree, Scheduler},
    utils, Candidate, Context, Engine, Vars, Workflow,
};
use rhai::Dynamic;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};

#[test]
fn env_run() {
    let env = Enviroment::new();

    let script = r#"
    let v = 5;
    print(`v=${v}`);
    "#;

    let result = env.run(script);

    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_eval() {
    let env = Enviroment::new();
    let script = r#"
    let v = 5;
    v
    "#;

    let result = env.eval::<i64>(script);

    assert_eq!(result.unwrap(), 5);
}

#[test]
fn env_eval_error() {
    let env = Enviroment::new();

    let script = r#"
    let v = 5
    v
    "#;

    let script_result = env.eval::<i64>(script);
    let reuslt = match script_result {
        Ok(..) => false,
        Err(_) => true,
    };

    assert_eq!(reuslt, true);
}

#[tokio::test]
async fn env_console_module() {
    let env = Enviroment::new();

    env.registry_console_module();
    let script = r#"
    let v = 5;
    console::log(`v=${v}`);
    console::dbg(`v=${v}`);
    console::wran(`v=${v}`);
    console::error(`v=${v}`);
    "#;
    let result = env.run(script);

    assert_eq!(result.unwrap(), true);
}

#[tokio::test]
async fn env_act_module() {
    let engine = Engine::new();
    engine.start();
    let env = Enviroment::new();
    env.registry_act_module();
    let room = env.new_room();

    let ctx = create_task_context();
    room.bind_context(&ctx);

    let script = r#"
    let a = 5;
    let b = 4;
    act.send("test1");

    true
    "#;
    let result = room.eval::<bool>(script);
    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_get() {
    let env = Enviroment::new();
    env.registry_env_module();

    let vars = Vars::from_iter([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    env.append(&vars);

    let script = r#"
    let a = env.get("a");
    a
    "#;
    let result = env.eval::<i64>(script);
    assert_eq!(result.unwrap(), 10);
}

#[test]
fn env_set() {
    let env = Enviroment::new();
    env.registry_env_module();

    let vars = Vars::from_iter([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    env.append(&vars);

    let script = r#"
    let a = env.get("a");
    a
    "#;
    let result = env.eval::<i64>(script);
    assert_eq!(result.unwrap(), 10);
}

#[test]
fn env_room_run() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
    let v = 5;
    print(`v=${v}`);
    "#;

    let result = room.run(script);
    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_room_eval() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
    let v = 5;
    v
    "#;

    let result = room.eval::<i64>(script);
    assert_eq!(result.unwrap(), 5);
}

#[test]
fn env_room_eval_error() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
    let v = 5
    v
    "#;

    let script_result = room.eval::<i64>(script);
    let reuslt = match script_result {
        Ok(..) => false,
        Err(_) => true,
    };
    assert_eq!(reuslt, true);
}

#[tokio::test]
async fn env_room_console_module() {
    let env = Enviroment::new();
    let room = env.new_room();
    env.registry_console_module();
    let script = r#"
    let v = 5;
    console::log(`v=${v}`);
    console::dbg(`v=${v}`);
    console::wran(`v=${v}`);
    console::error(`v=${v}`);
    "#;
    let result = room.run(script);
    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_room_get() {
    let env = Enviroment::new();
    let room = env.new_room();
    env.registry_env_module();

    let vars = Vars::from_iter([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    room.append(&vars);

    let script = r#"
    let a = env.get("a");
    a
    "#;
    let result = room.eval::<i64>(script);
    assert_eq!(result.unwrap(), 10);
}

#[test]
fn env_room_set() {
    let env = Enviroment::new();
    let room = env.new_room();
    env.registry_env_module();

    let vars = Vars::from_iter([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    room.append(&vars);

    let script = r#"
    let a = env.get("a");

    env.set("a", a + 5);
    a
    "#;
    let result = room.eval::<i64>(script);
    assert_eq!(result.unwrap(), vars.get("a").unwrap().as_i64().unwrap());
}

#[test]
fn env_room_share_global_vars() {
    let env = Enviroment::new();

    env.set("abc", json!(1.5));
    let room = env.new_room();
    let script = r#"
    let v = 5;
    let v2 = env.get("abc");
    v2
    "#;

    let result = room.eval::<f64>(script);
    assert_eq!(result.unwrap(), 1.5);
}

#[test]
fn env_room_override_global_vars() {
    let env = Enviroment::new();
    env.set("abc", json!(1.5));

    let room = env.new_room();
    room.set("abc", json!("Tom"));
    assert_eq!(room.get("abc").unwrap(), "Tom");

    let script = r#"
    let v = 5;
    let v2 = env.get("abc");
    v2
    "#;

    let result = env.eval::<String>(script);
    assert_eq!(result.unwrap(), "Tom");
}

#[test]
fn env_room_output() {
    let env = Enviroment::new();
    let room = env.new_room();

    let mut vars = Vars::new();
    vars.insert("output".to_string(), 100.into());
    room.output(&vars);
    assert_eq!(env.get("output").unwrap(), 100);
}

#[test]
fn env_room_get_error() {
    let env = Enviroment::new();
    let room = env.new_room();

    let script = r#"
        let v = env.get("abc");
        return v;
    "#;

    let result = room.eval::<String>(script);
    assert_eq!(result.is_err(), true);
}

#[test]
fn env_room_over_env() {
    let env = Enviroment::new();
    let room = env.new_room();
    env.set("a", 100.into());
    room.set("a", 10.into());
    let script = r#"
        let v = env.get("a");
        return v;
    "#;

    let result = room.eval::<i64>(script).unwrap();
    assert_eq!(result, 10);
}

#[test]
fn env_collection_array() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
        return ["a", "b"];
    "#;

    let result = room.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "set");
    assert_eq!(
        cand.users(&HashMap::new())
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn env_collection_union() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
        let a = ["a"];
        let b = ["b"];
        return a.union(b);
    "#;

    let result = room.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "group");
    assert_eq!(
        cand.users(&HashMap::new())
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn env_collection_intersect() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
        let a = ["a", "b"];
        let b = ["b", "c"];
        return a.intersect(b);
    "#;

    let result = room.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "group");
    assert_eq!(
        cand.users(&HashMap::new())
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn env_collection_diff() {
    let env = Enviroment::new();
    let room = env.new_room();
    let script = r#"
        let a = ["a", "b"];
        let b = ["b"];
        return a.diff(b);
    "#;

    let result = room.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "group");
    assert_eq!(
        cand.users(&HashMap::new())
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["a"]
    );
}

fn create_task_context() -> Arc<Context> {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1")
            .with_step(|step| step.with_name("step1"))
    });
    let id = utils::longid();
    let tr = NodeTree::build(&mut workflow);
    let scher = Scheduler::new();
    let proc = scher.create_proc(&id, &workflow);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None);

    task.create_context(&scher)
}
