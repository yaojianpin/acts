use crate::{Act, If, StmtBuild, Vars};

#[test]
fn model_act_if_parse() {
    let text = r#"
    act: if
    inputs:
        on: $("a") > 0
        then:
        - act: msg
          inputs:
            key: msg1
    "#;
    if let Ok(Act { act, inputs, .. }) = serde_yaml::from_str(text) {
        assert_eq!(act, "if");
        assert_eq!(inputs.get::<String>("on").unwrap(), r#"$("a") > 0"#);
        assert_eq!(inputs.get::<Vec<Act>>("then").unwrap().len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_if_on() {
    let act = If::new().with_on(r#"$("a") > 0"#);
    assert_eq!(act.on, r#"$("a") > 0"#);
}

#[test]
fn model_act_if_then() {
    let act = If::new().with_then(|stmts| stmts.add(Act::set(Vars::new())));
    assert_eq!(act.then.len(), 1);
}

#[test]
fn model_act_if_else() {
    let act = If::new().with_else(|stmts| stmts.add(Act::set(Vars::new())));
    assert_eq!(act.r#else.len(), 1);
}
