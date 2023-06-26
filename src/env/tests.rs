use crate::{
    env::Enviroment,
    sch::{NodeTree, Scheduler},
    utils, Context, Engine, Vars, Workflow,
};
use serde_json::json;
use std::sync::Arc;

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
    let vm = env.vm();

    let ctx = create_task_context();
    vm.bind_context(&ctx);

    let script = r#"
    let a = 5;
    let b = 4;
    act.send("test1");

    true
    "#;
    let result = vm.eval::<bool>(script);

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
fn env_vm_run() {
    let env = Enviroment::new();
    let vm = env.vm();
    let script = r#"
    let v = 5;
    print(`v=${v}`);
    "#;

    let result = vm.run(script);

    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_vm_eval() {
    let env = Enviroment::new();
    let vm = env.vm();
    let script = r#"
    let v = 5;
    v
    "#;

    let result = vm.eval::<i64>(script);

    assert_eq!(result.unwrap(), 5);
}

#[test]
fn env_vm_eval_error() {
    let env = Enviroment::new();
    let vm = env.vm();
    let script = r#"
    let v = 5
    v
    "#;

    let script_result = vm.eval::<i64>(script);
    let reuslt = match script_result {
        Ok(..) => false,
        Err(_) => true,
    };

    assert_eq!(reuslt, true);
}

#[tokio::test]
async fn env_vm_console_module() {
    let env = Enviroment::new();
    let vm = env.vm();
    env.registry_console_module();
    let script = r#"
    let v = 5;
    console::log(`v=${v}`);
    console::dbg(`v=${v}`);
    console::wran(`v=${v}`);
    console::error(`v=${v}`);
    "#;
    let result = vm.run(script);

    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_vm_get() {
    let env = Enviroment::new();
    let vm = env.vm();
    env.registry_env_module();

    let vars = Vars::from_iter([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    vm.append(&vars);

    let script = r#"
    let a = env.get("a");
    a
    "#;
    let result = vm.eval::<i64>(script);

    assert_eq!(result.unwrap(), 10);
}

#[test]
fn env_vm_set() {
    let env = Enviroment::new();
    let vm = env.vm();
    env.registry_env_module();

    let vars = Vars::from_iter([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    vm.append(&vars);

    let script = r#"
    let a = env.get("a");

    env.set("a", a + 5);
    a
    "#;
    let result = vm.eval::<i64>(script);

    assert_eq!(result.unwrap(), vars.get("a").unwrap().as_i64().unwrap());
}

#[test]
fn env_vm_share_global_vars() {
    let env = Enviroment::new();

    env.set("abc", json!(1.5));
    let vm = env.vm();
    let script = r#"
    let v = 5;
    let v2 = env.get("abc");
    v2
    "#;

    let result = vm.eval::<f64>(script);
    assert_eq!(result.unwrap(), 1.5);
}

#[test]
fn env_vm_override_global_vars() {
    let env = Enviroment::new();
    env.set("abc", json!(1.5));

    let vm = env.vm();
    vm.set("abc", json!("Tom"));
    assert_eq!(vm.get("abc").unwrap(), "Tom");

    let script = r#"
    let v = 5;
    let v2 = env.get("abc");
    v2
    "#;

    let result = env.eval::<String>(script);
    assert_eq!(result.unwrap(), "Tom");
}

#[test]
fn env_vm_output() {
    let env = Enviroment::new();
    let vm = env.vm();

    let mut vars = Vars::new();
    vars.insert("output".to_string(), 100.into());
    vm.output(&vars);

    assert_eq!(env.get("output").unwrap(), 100);
}

#[test]
fn env_vm_get_error() {
    let env = Enviroment::new();
    let vm = env.vm();

    let script = r#"
        let v = env.get("abc");
        return v;
    "#;

    let result = vm.eval::<String>(script);
    assert_eq!(result.is_err(), true);
}

fn create_task_context() -> Arc<Context> {
    let text = include_str!("../sch/tests/models/simple.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    let id = utils::longid();

    let tr = NodeTree::build(&mut workflow);
    let scher = Scheduler::new();
    let proc = scher.create_proc(&id, &workflow);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None);

    task.create_context(&scher)
}
