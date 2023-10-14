use crate::Act;
use serde_json::json;

#[test]
fn model_act_id() {
    let act = Act::new().with_id("act1");
    assert_eq!(act.id, "act1");
}

#[test]
fn model_act_name() {
    let act = Act::new().with_name("my name");
    assert_eq!(act.name, "my name");
}

#[test]
fn model_act_inputs() {
    let act = Act::new().with_input("p1", json!(5));
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(&json!(5)));
}

#[test]
fn model_act_outputs() {
    let act = Act::new().with_output("p1", json!(5));
    assert_eq!(act.outputs.len(), 1);
    assert!(act.outputs.get("p1").is_some());
}

#[test]
fn model_act_tag() {
    let act = Act::new().with_tag("tag1");
    assert_eq!(act.tag, "tag1");
}

#[test]
fn model_act_for() {
    let mut act = Act::new();
    assert!(act.r#for.is_none());

    act = act.with_for(|f| {
        f.with_by("all")
            .with_alias(|a| a.with_init("my_init").with_each("my_each"))
            .with_in(r#"print("in")"#)
    });

    let f = act.r#for.unwrap();
    assert_eq!(f.by, "all");
    assert_eq!(f.alias.init.unwrap(), "my_init");
    assert_eq!(f.alias.each.unwrap(), "my_each");
    assert_eq!(f.r#in.is_empty(), false);
}
