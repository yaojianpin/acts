use std::collections::HashMap;

use crate::{env::Enviroment, Engine};

#[test]
fn env_run() {
    let env = Enviroment::new();

    let script = r#"
    let v = 5;
    print(`v=${v}`);
    "#;

    let vm = env.vm();
    let result = env.run(script, &vm);

    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_eval() {
    let env = Enviroment::new();
    let script = r#"
    let v = 5;
    v
    "#;

    let vm = env.vm();
    let result = env.eval::<i64>(script, &vm);

    assert_eq!(result.unwrap(), 5);
}

#[test]
fn env_eval_error() {
    let env = Enviroment::new();

    let script = r#"
    let v = 5
    v
    "#;

    let vm = env.vm();
    let script_result = env.eval::<i64>(script, &vm);
    let reuslt = match script_result {
        Ok(..) => false,
        Err(_) => true,
    };

    assert_eq!(reuslt, true);
}

#[tokio::test]
async fn env_console_module() {
    let engine = Engine::new();
    let env = Enviroment::new();

    env.registry_console_module(&engine);
    let vm = env.vm();
    let script = r#"
    let v = 5;
    console::log(`v=${v}`);
    console::dbg(`v=${v}`);
    console::wran(`v=${v}`);
    console::error(`v=${v}`);
    "#;
    let result = env.run(script, &vm);

    assert_eq!(result.unwrap(), true);
}

#[tokio::test]
async fn env_act_module() {
    let mut engine = Engine::new();

    let add = |a: i64, b: i64| Ok(a + b);
    engine.register_action("add", add);

    let env = Enviroment::new();
    env.registry_act_module(&engine);

    let vm = env.vm();

    let script = r#"
    let a = 5;
    let b = 4;
    let result = yao::add(a, b);

    result
    "#;
    let result = env.eval::<i64>(script, &vm);

    assert_eq!(result.unwrap(), 9);
}

#[test]
fn env_vm_get() {
    let engine = Engine::new();
    let env = Enviroment::new();
    env.registry_env_module(&engine);

    let vm = env.vm();

    let vars = HashMap::from([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    vm.append(vars);

    let script = r#"
    let a = env.get("a");
    a
    "#;
    let result = vm.eval::<i64>(script);

    assert_eq!(result.unwrap(), 10);
}

#[test]
fn env_vm_set() {
    let engine = Engine::new();
    let env = Enviroment::new();
    env.registry_env_module(&engine);

    let vm = env.vm();

    let vars = HashMap::from([("a".to_string(), 10.into()), ("b".to_string(), "b".into())]);
    vm.append(vars);

    let script = r#"
    let a = env.get("a");
    a
    "#;
    let result = vm.eval::<i64>(script);

    assert_eq!(result.unwrap(), 10);
}
