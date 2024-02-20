use crate::{Act, Each, StmtBuild, Vars};

#[test]
fn model_act_each_parse() {
    let text = r#"
    !each
    in: "[\"a\", \"b\"]"
    run:
      - !msg
        id: msg1
    "#;
    if let Act::Each(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.r#in, r#"["a", "b"]"#);
        assert_eq!(stmt.run.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_each_in() {
    let act = Each::new().with_in(r#"["u1"]"#);
    assert_eq!(act.r#in, r#"["u1"]"#);
}

#[test]
fn model_act_each_run() {
    let act = Each::new().with_run(|stmts| stmts.add(Act::set(Vars::new())));
    assert_eq!(act.run.len(), 1);
}
