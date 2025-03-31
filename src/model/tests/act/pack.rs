use crate::{Act, Pack};
use serde_json::json;

#[test]
fn model_act_pack_parse() {
    let text = r#"
    act: pack
    key: p1
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
        assert_eq!(act, "pack");
        assert_eq!(key, "p1");
        assert_eq!(inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(rets.get_value("a").unwrap(), &json!(null));
    } else {
        panic!();
    }
}

#[test]
fn model_act_pack_key() {
    let act = Pack::new().with_key("m1");
    assert_eq!(act.key, "m1");
}

#[test]
fn model_act_pack_inputs() {
    let act = Pack::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}

#[test]
fn model_act_pack_rets() {
    let act = Pack::new().with_output("p1", 5);
    assert_eq!(act.outputs.len(), 1);
    assert!(act.outputs.get_value("p1").is_some());
}
