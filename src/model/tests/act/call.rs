use crate::{Act, Call};
use serde_json::json;

#[test]
fn model_act_call_parse() {
    let text = r#"
    act: call
    key: m1
    inputs:
        a: 5
    rets:
      a:
    "#;
    if let Ok(Act {
        act,
        inputs,
        key,
        rets,
        ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "call");
        assert_eq!(key, "m1");
        assert_eq!(inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(rets.get_value("a").unwrap(), &json!(null));
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_call_key() {
    let act = Call::new().with_key("m1");
    assert_eq!(act.key, "m1");
}

#[test]
fn model_act_call_with() {
    let act = Call::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}

#[test]
fn model_act_call_rets() {
    let act = Call::new().with_ret("p1", 5);
    assert_eq!(act.rets.len(), 1);
    assert!(act.rets.get_value("p1").is_some());
}
