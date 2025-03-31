use crate::{Act, Do, Error};

#[test]
fn model_act_do_parse() {
    let text = r#"
    act: do
    key: error
    inputs:
      error:
        ecode: err1
        message: abc
    "#;
    if let Ok(Act {
        act, key, inputs, ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "do");
        assert_eq!(key, r#"error"#);

        let err = inputs.get::<Error>("error").unwrap();
        assert_eq!(err.ecode, "err1");
        assert_eq!(err.message, "abc");
    } else {
        panic!();
    }
}

#[test]
fn model_act_do_name() {
    let act = Do::new().with_key(r#"next"#);
    assert_eq!(act.key, r#"next"#);
}

#[test]
fn model_act_do_inputs() {
    let act = Do::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get("p1"), Some(5));
}
