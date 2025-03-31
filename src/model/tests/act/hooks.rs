use crate::Act;

#[test]
fn model_act_parse_on_created() {
    let text = r#"
    act: on_created
    then:
      - act: msg
        key: msg1
    "#;
    if let Ok(Act { act, then, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "on_created");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_completed() {
    let text = r#"
    act: on_completed
    then:
        - act: msg
          key: msg1
    "#;
    if let Ok(Act { act, then, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "on_completed");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_updated() {
    let text = r#"
    act: on_updated
    then:
        - act: msg
          key: msg1
    "#;
    if let Ok(Act { act, then, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "on_updated");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_before_update() {
    let text = r#"
    act: on_before_update
    then:
      - act: msg
        key: msg1
    "#;
    if let Ok(Act { act, then, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "on_before_update");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_step() {
    let text = r#"
    act: on_step
    then:
      - act: msg
        key: msg1
    "#;
    if let Ok(Act { act, then, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "on_step");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_timeout() {
    let text = r#"
    act: on_timeout
    timeout:
      - on: 2d
        then:
            - act: msg
              key: msg1
    "#;
    if let Ok(Act { timeout, .. }) = serde_yaml::from_str(text) {
        assert_eq!(timeout.len(), 1);
        let timeout = timeout.first().unwrap();
        assert_eq!(timeout.on.value, 2);
        assert_eq!(timeout.then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_act_parse_on_error_catch() {
    let text = r#"
    act: on_catch
    catches:
      - on: err1
        then:
          - act: msg
            key: msg1
    "#;
    if let Ok(Act { act, catches, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "on_catch");
        assert_eq!(catches.len(), 1);
        let catch = catches.first().unwrap();
        assert_eq!(catch.on.as_ref().unwrap(), "err1");
        assert_eq!(catch.then.len(), 1);
    } else {
        panic!();
    }
}
