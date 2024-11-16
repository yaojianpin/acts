use crate::{Act, Chain, StmtBuild, Vars};

#[test]
fn model_act_chain_parse() {
    let text = r#"
    act: chain
    in: "[\"a\", \"b\"]"
    then:
        - act: msg
          key: msg1
    "#;
    if let Ok(Act {
        act, r#in, then, ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "chain");
        assert_eq!(r#in, r#"["a", "b"]"#);
        assert_eq!(then.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_chain_in() {
    let act = Chain::new().with_in(r#"["u1"]"#);
    assert_eq!(act.r#in, r#"["u1"]"#);
}

#[test]
fn model_act_chain_run() {
    let act = Chain::new().with_then(|stmts| stmts.add(Act::set(Vars::new())));
    assert_eq!(act.then.len(), 1);
}
