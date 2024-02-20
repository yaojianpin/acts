use crate::{Act, Use};
use serde_json::json;

#[test]
fn model_act_use_parse() {
    let text = r#"
    !use
    id: use1
    mid: m1
    inputs:
      a: 5
    outputs:
      a:
    "#;
    if let Act::Use(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.id, "use1");
        assert_eq!(stmt.mid, "m1");
        assert_eq!(stmt.inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(stmt.outputs.get_value("a").unwrap(), &json!(null));
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_use_id() {
    let act = Use::new().with_id("act1");
    assert_eq!(act.id, "act1");
}

#[test]
fn model_act_use_mid() {
    let act = Use::new().with_mid("m1");
    assert_eq!(act.mid, "m1");
}

#[test]
fn model_act_use_inputs() {
    let act = Use::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}

#[test]
fn model_act_use_outputs() {
    let act = Use::new().with_output("p1", 5);
    assert_eq!(act.outputs.len(), 1);
    assert!(act.outputs.get_value("p1").is_some());
}
