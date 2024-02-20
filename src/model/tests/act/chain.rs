use crate::{Act, Chain, StmtBuild, Vars};

#[test]
fn model_act_chain_parse() {
    let text = r#"
    !chain
    in: "[\"a\", \"b\"]"
    run:
      - !msg
        id: msg1
    "#;
    if let Act::Chain(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.r#in, r#"["a", "b"]"#);
        assert_eq!(stmt.run.len(), 1);
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
    let act = Chain::new().with_run(|stmts| stmts.add(Act::set(Vars::new())));
    assert_eq!(act.run.len(), 1);
}
