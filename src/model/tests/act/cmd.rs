use crate::{Act, Cmd};

#[test]
fn model_act_cmd_parse() {
    let text = r#"
    !cmd
    name: error
    inputs:
      err_code: err1
      err_message: abc
    "#;
    if let Act::Cmd(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.name, r#"error"#);
        assert_eq!(stmt.inputs.get::<String>("err_code").unwrap(), "err1");
        assert_eq!(stmt.inputs.get::<String>("err_message").unwrap(), "abc");
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_cmd_name() {
    let act = Cmd::new().with_name(r#"complete"#);
    assert_eq!(act.name, r#"complete"#);
}

#[test]
fn model_act_cmd_inputs() {
    let act = Cmd::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}
