use crate::{Act, Each, StmtBuild, Vars};

#[test]
fn model_act_each_parse() {
    let text = r#"
    act: each
    in: "[\"a\", \"b\"]"
    then:
        - act: msg
          key: msg1
    "#;
    if let Ok(Act {
        act, r#in, then, ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "each");
        assert_eq!(r#in, r#"["a", "b"]"#);
        assert_eq!(then.len(), 1);
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
    let act = Each::new().with_then(|stmts| stmts.add(Act::set(Vars::new())));
    assert_eq!(act.then.len(), 1);
}
