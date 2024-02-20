use crate::{Act, Req};
use serde_json::json;

#[test]
fn model_act_parse_req() {
    let text = r#"
    !req
    id: msg1
    tag: tag1
    inputs:
        a: 1
    outputs:
        b:
    "#;
    if let Act::Req(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.id, "msg1");
        assert_eq!(stmt.tag, "tag1");
        assert_eq!(stmt.inputs.get::<i32>("a").unwrap(), 1);
        assert_eq!(stmt.outputs.get_value("b").unwrap(), &json!(null));
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_req_id() {
    let act = Req::new().with_id("act1");
    assert_eq!(act.id, "act1");
}

#[test]
fn model_act_req_name() {
    let act = Req::new().with_name("my name");
    assert_eq!(act.name, "my name");
}

#[test]
fn model_act_req_inputs() {
    let act = Req::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}

#[test]
fn model_act_req_outputs() {
    let act = Req::new().with_output("p1", 5);
    assert_eq!(act.outputs.len(), 1);
    assert!(act.outputs.get_value("p1").is_some());
}

#[test]
fn model_act_req_tag() {
    let act = Req::new().with_tag("tag1");
    assert_eq!(act.tag, "tag1");
}
