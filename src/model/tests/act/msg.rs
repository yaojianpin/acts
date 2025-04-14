use crate::{Act, Msg};

#[test]
fn model_act_msg_parse() {
    let text = r#"
    act: msg
    inputs:
      key: msg1
      a: 1
    tag: tag1
    "#;
    if let Ok(Act {
        act, tag, inputs, ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "msg");
        assert_eq!(inputs.get::<String>("key").unwrap(), "msg1");
        assert_eq!(tag, "tag1");
        assert_eq!(inputs.get::<i32>("a").unwrap(), 1);
    } else {
        panic!();
    }
}

// #[test]
// fn model_act_msg_id() {
//     let act = Msg::new().with_id("act1");
//     assert_eq!(act.id, "act1");
// }

// #[test]
// fn model_act_msg_name() {
//     let act = Msg::new().with_name("my name");
//     assert_eq!(act.name, "my name");
// }

#[test]
fn model_act_msg_with() {
    let act = Msg::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}

#[test]
fn model_act_msg_tag() {
    let act = Msg::new().with_tag("tag1");
    assert_eq!(act.tag, "tag1");
}

#[test]
fn model_act_msg_key() {
    let act = Msg::new().with_key("key1");
    assert_eq!(act.key, "key1");
}
