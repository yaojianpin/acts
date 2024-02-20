use crate::{env::Enviroment, utils::consts, Candidate, Vars};
use rhai::Dynamic;
use serde_json::{json, Value};

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

#[test]
fn env_get_set() {
    let env = Enviroment::new();

    env.set("a", 5);
    assert_eq!(env.get::<i64>("a").unwrap(), 5);
    assert_eq!(env.get::<i32>("a").unwrap(), 5);

    env.set("a", false);
    assert_eq!(env.get::<bool>("a").unwrap(), false);

    env.set("a", "abc");
    assert_eq!(env.get::<String>("a").unwrap(), "abc");

    env.set("a", 10.56);
    assert_eq!(env.get::<f64>("a").unwrap(), 10.56);
    assert_eq!(env.get::<f32>("a").unwrap(), 10.56);

    env.set("a", json!(100));
    assert_eq!(env.get::<Value>("a").unwrap(), json!(100));
    assert_eq!(env.get::<i32>("a").unwrap(), 100);
}

#[test]
fn env_get_not_exists() {
    let env = Enviroment::new();
    assert_eq!(env.get_value("a"), None);
}

#[test]
fn env_get_set_data() {
    let env = Enviroment::new();
    env.set_data("test1", &Vars::new().with("a", 5));
    assert_eq!(env.data("test1").unwrap().get::<i32>("a").unwrap(), 5);

    env.set_data("test1", &Vars::new().with("a", 10));
    assert_eq!(env.data("test1").unwrap().get::<i32>("a").unwrap(), 10);
}

#[test]
fn env_update_data() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    refenv.set("a", 0);

    let vars = Vars::new().with("a", 5).with("b", "abc");
    let mut not_updated = Vec::new();
    for (k, v) in &vars {
        let ret = env.update_data("test1", k, v);
        if !ret {
            not_updated.push(k);
        }
    }

    assert_eq!(env.data("test1").unwrap().get::<i32>("a").unwrap(), 5);
    assert_eq!(env.data("test1").unwrap().get::<i32>("b"), None);
    assert_eq!(not_updated, ["b"]);
}

#[test]
fn env_remove() {
    let env = Enviroment::new();
    env.set("a", 10);
    assert_eq!(env.get::<i32>("a").unwrap(), 10);

    env.remove("a");
    assert!(env.get::<i32>("a").is_none());
}

#[test]
fn env_ref_get_set() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");

    refenv.set("a", 5);
    assert_eq!(refenv.get::<i64>("a").unwrap(), 5);
    assert_eq!(refenv.get::<i32>("a").unwrap(), 5);

    refenv.set("a", false);
    assert_eq!(refenv.get::<bool>("a").unwrap(), false);

    refenv.set("a", "abc");
    assert_eq!(refenv.get::<String>("a").unwrap(), "abc");

    refenv.set("a", 10.56);
    assert_eq!(refenv.get::<f64>("a").unwrap(), 10.56);
    assert_eq!(refenv.get::<f32>("a").unwrap(), 10.56);

    refenv.set("a", json!(100));
    assert_eq!(refenv.get::<Value>("a").unwrap(), json!(100));
    assert_eq!(refenv.get::<i32>("a").unwrap(), 100);
}

#[test]
fn env_ref_data() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");

    refenv.set("a", 5);
    assert_eq!(refenv.data().get::<i32>("a").unwrap(), 5);
}

#[test]
fn env_ref_update_tree_top() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    refenv.set("a", 1);

    let refenv2 = env.create_ref("test2");
    refenv2.set(consts::ENV_PARENT_TASK_ID, "test1");
    refenv2.set("b", 1);

    let refenv3 = env.create_ref("test3");
    refenv3.set(consts::ENV_PARENT_TASK_ID, "test2");
    refenv3.set("c", 1);

    refenv.set_env(&Vars::new().with("a", 5).with("b", 5).with("c", 5));
    assert_eq!(env.data("test1").unwrap().get::<i32>("a").unwrap(), 5);
    assert_eq!(env.data("test1").unwrap().get::<i32>("b").unwrap(), 5);
    assert_eq!(env.data("test1").unwrap().get::<i32>("c").unwrap(), 5);
    assert_eq!(env.data("test2").unwrap().get::<i32>("b").unwrap(), 1);
    assert_eq!(env.data("test3").unwrap().get::<i32>("c").unwrap(), 1);
}

#[test]
fn env_ref_update_tree_sub() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    refenv.set("a", 1);

    let refenv2 = env.create_ref("test2");
    refenv2.set(consts::ENV_PARENT_TASK_ID, "test1");
    refenv2.set("b", 1);

    let refenv3 = env.create_ref("test3");
    refenv3.set(consts::ENV_PARENT_TASK_ID, "test2");
    refenv3.set("c", 1);

    refenv3.set_env(
        &Vars::new()
            .with("a", 20)
            .with("b", 20)
            .with("c", 20)
            .with("d", 20),
    );
    assert_eq!(env.data("test1").unwrap().get::<i32>("a").unwrap(), 20);
    assert_eq!(env.data("test2").unwrap().get::<i32>("b").unwrap(), 20);
    assert_eq!(env.data("test3").unwrap().get::<i32>("c").unwrap(), 20);
    assert_eq!(env.data("test3").unwrap().get::<i32>("d").unwrap(), 20);
}

#[test]
fn env_ref_run() {
    let env = Enviroment::new();
    let room = env.create_ref("test1");
    let script = r#"
    let v = 5;
    print(`v=${v}`);
    "#;

    let result = room.run(script);
    assert_eq!(result.unwrap(), true);
}

#[test]
fn env_ref_eval() {
    let env = Enviroment::new();
    let room = env.create_ref("test1");
    let script = r#"
    let v = 5;
    v
    "#;

    let result = room.eval::<i64>(script);
    assert_eq!(result.unwrap(), 5);
}

#[test]
fn env_ref_eval_error() {
    let env = Enviroment::new();
    let room = env.create_ref("test1");
    let script = r#"
    let v = 5  // needs end with ;
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
async fn env_ref_console_module() {
    let env = Enviroment::new();
    let room = env.create_ref("test1");
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
fn env_ref_create() {
    let env = Enviroment::new();
    env.create_ref("test1");

    assert!(env.data("test1").is_some());
}

#[test]
fn env_ref_remove() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");

    refenv.set("a", 10);
    refenv.set("b", "abc");
    assert_eq!(refenv.get::<i32>("a").unwrap(), 10);
    assert_eq!(refenv.get::<String>("b").unwrap(), "abc");

    let data = env.data("test1").unwrap();
    assert_eq!(data.get::<i32>("a").unwrap(), 10);
    assert_eq!(data.get::<String>("b").unwrap(), "abc");

    refenv.remove("a");
    assert!(data.get::<String>("a").is_none());
}

#[test]
fn env_ref_throw_error() {
    let env = Enviroment::new();
    let room = env.create_ref("test1");

    let script = r#"
        throw "error"
    "#;

    let result = room.eval::<String>(script);
    assert_eq!(result.is_err(), true);
}

#[test]
fn env_collection_array() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    let script = r#"
        return ["a", "b"];
    "#;

    let result = refenv.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "set");
    assert_eq!(
        cand.values().unwrap().into_iter().collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn env_collection_union() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    let script = r#"
        let a = ["a"];
        let b = ["b"];
        return a.union(b);
    "#;

    let result = refenv.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "group");
    assert_eq!(
        cand.values().unwrap().into_iter().collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn env_collection_intersect() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    let script = r#"
        let a = ["a", "b"];
        let b = ["b", "c"];
        return a.intersect(b);
    "#;

    let result = refenv.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "group");
    assert_eq!(
        cand.values().unwrap().into_iter().collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn env_collection_except() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");
    let script = r#"
        let a = ["a", "b"];
        let b = ["b"];
        return a.except(b);
    "#;

    let result = refenv.eval::<Dynamic>(script).unwrap();
    let cand = Candidate::parse(&result.to_string()).unwrap();
    assert_eq!(cand.r#type(), "group");
    assert_eq!(
        cand.values().unwrap().into_iter().collect::<Vec<_>>(),
        ["a"]
    );
}

#[test]
fn env_get_get_value_by_type() {
    let env = Enviroment::new();

    env.set("a", 5);
    assert_eq!(env.get::<u32>("a").unwrap(), 5);
    assert_eq!(env.get::<String>("a"), None);
}

#[test]
fn env_ref_get_get_value_diff_type() {
    let env = Enviroment::new();
    let refenv = env.create_ref("test1");

    refenv.set("a", 5);
    assert_eq!(refenv.get::<u32>("a").unwrap(), 5);
    assert_eq!(refenv.get::<String>("a"), None);
}
