use crate::{Act, Cmd, Error};

#[test]
fn model_act_cmd_parse() {
    let text = r#"
    !cmd
    name: error
    inputs:
      error:
        ecode: err1
        message: abc
    "#;
    if let Act::Cmd(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.name, r#"error"#);

        let err = stmt.inputs.get::<Error>("error").unwrap();
        assert_eq!(err.ecode, "err1");
        assert_eq!(err.message, "abc");
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_cmd_name() {
    let act = Cmd::new().with_name(r#"next"#);
    assert_eq!(act.name, r#"next"#);
}

#[test]
fn model_act_cmd_inputs() {
    let act = Cmd::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}
