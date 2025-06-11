use crate::{Act, ActEvent};

#[test]
fn model_act_parse_on_created() {
    let text = r#"
    on: created
    uses: acts.core.msg
    key: msg1
    "#;
    if let Ok(Act { uses, on, key, .. }) = serde_yaml::from_str(text) {
        assert_eq!(uses, "acts.core.msg");
        assert_eq!(on.unwrap(), ActEvent::Created);
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_completed() {
    let text = r#"
    on: completed
    uses: acts.core.msg
    key: msg1
    "#;
    if let Ok(Act { uses, on, key, .. }) = serde_yaml::from_str(text) {
        assert_eq!(uses, "acts.core.msg");
        assert_eq!(on.unwrap(), ActEvent::Completed);
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_updated() {
    let text = r#"
    on: updated
    uses: acts.core.msg
    key: msg1
    "#;
    if let Ok(Act { uses, on, key, .. }) = serde_yaml::from_str(text) {
        assert_eq!(uses, "acts.core.msg");
        assert_eq!(on.unwrap(), ActEvent::Updated);
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_before_update() {
    let text = r#"
    on: before_update
    uses: acts.core.msg
    key: msg1
    "#;
    if let Ok(Act { uses, on, key, .. }) = serde_yaml::from_str(text) {
        assert_eq!(uses, "acts.core.msg");
        assert_eq!(on.unwrap(), ActEvent::BeforeUpdate);
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_step() {
    let text = r#"
    on: step
    uses: acts.core.msg
    key: msg1
    "#;
    if let Ok(Act { uses, on, key, .. }) = serde_yaml::from_str(text) {
        assert_eq!(uses, "acts.core.msg");
        assert_eq!(on.unwrap(), ActEvent::Step);
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}
