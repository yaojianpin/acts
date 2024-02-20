use crate::Vars;
use serde_json::json;

#[test]
fn model_vars_new() {
    let vars = Vars::new();
    assert_eq!(vars.len(), 0);
}

#[test]
fn model_vars_insert() {
    let mut vars = Vars::new();
    vars.insert("a".to_string(), json!(10));
    assert_eq!(vars.get_value("a").unwrap(), &json!(10));
}

#[test]
fn model_vars_set() {
    let mut vars = Vars::new();
    vars.set("a", json!(10));
    assert_eq!(vars.get_value("a").unwrap(), &json!(10));
}

#[test]
fn model_vars_set_vec() {
    let mut vars = Vars::new();
    vars.set("a", ["a"]);
    assert_eq!(vars.get::<Vec<String>>("a").unwrap(), ["a"]);
}

#[test]
fn model_vars_remove() {
    let mut vars = Vars::new();
    vars.set("a", json!(10));
    vars.remove("a");
    assert_eq!(vars.get_value("a"), None);
}

#[test]
fn model_vars_with() {
    let vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(vars.len(), 2);
    assert_eq!(vars.get_value("a").unwrap(), &json!(10));
    assert_eq!(vars.get_value("b").unwrap(), &json!("text"));
}

#[test]
fn model_vars_iter() {
    let vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(vars.iter().len(), 2);
}

#[test]
fn model_vars_iter_mut() {
    let mut vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(vars.iter_mut().len(), 2);
}

#[test]
fn model_vars_to_string() {
    let vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(
        vars.to_string(),
        json!({ "a": 10, "b": "text" }).to_string()
    );
}
