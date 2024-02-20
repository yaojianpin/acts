use crate::Act;

#[test]
fn model_act_parse_on_created() {
    let text = r#"
    !on_created
    - !msg
      id: msg1
    "#;
    if let Act::OnCreated(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_on_completed() {
    let text = r#"
    !on_completed
    - !msg
      id: msg1
    "#;
    if let Act::OnCompleted(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_on_updated() {
    let text = r#"
    !on_updated
    - !msg
      id: msg1
    "#;
    if let Act::OnUpdated(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_on_before_update() {
    let text = r#"
    !on_before_update
    - !msg
      id: msg1
    "#;
    if let Act::OnBeforeUpdate(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_on_step() {
    let text = r#"
    !on_step
    - !msg
      id: msg1
    "#;
    if let Act::OnStep(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_on_timeout() {
    let text = r#"
    !on_timeout
    - on: 2d
      then:
        - !msg
          id: msg1
    "#;
    if let Act::OnTimeout(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
        let timeout = stmts.get(0).unwrap();
        assert_eq!(timeout.on.value, 2);
        assert_eq!(timeout.then.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_on_error_catch() {
    let text = r#"
    !on_error_catch
    - err: err1
      then:
        - !msg
          id: msg1
    "#;
    if let Act::OnErrorCatch(stmts) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmts.len(), 1);
        let catch = stmts.get(0).unwrap();
        assert_eq!(catch.err.as_ref().unwrap(), "err1");
        assert_eq!(catch.then.len(), 1);
    } else {
        assert!(false);
    }
}
