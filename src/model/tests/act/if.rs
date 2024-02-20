use crate::{Act, If, StmtBuild, Vars};

#[test]
fn model_act_if_parse() {
    let text = r#"
    !if
    on: env.get("a") > 0
    then:
      - !msg
        id: msg1
    "#;
    if let Act::If(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.on, r#"env.get("a") > 0"#);
        assert_eq!(stmt.then.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_if_on() {
    let act = If::new().with_on(r#"env.get("a") > 0"#);
    assert_eq!(act.on, r#"env.get("a") > 0"#);
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
