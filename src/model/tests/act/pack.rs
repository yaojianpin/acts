use crate::{Act, Package, StmtBuild};

#[test]
fn model_act_pack_parse() {
    let text = r#"
    !pack
    id: pack1
    acts:
      - !msg
        id: msg1
    inputs:
      a: 5
    next:
      id: pack2
    "#;
    if let Act::Package(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.acts.len(), 1);
        assert_eq!(stmt.inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(stmt.next.as_ref().unwrap().id, "pack2");
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_pack_id() {
    let act = Package::new().with_id("pack1");
    assert_eq!(act.id, "pack1");
}

#[test]
fn model_act_pack_input() {
    let act = Package::new().with_input("a", 5);
    assert_eq!(act.inputs.get::<i32>("a").unwrap(), 5);
}

#[test]
fn model_act_pack_acts() {
    let act = Package::new().with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))));

    assert_eq!(act.acts.len(), 1);
}

#[test]
fn model_act_pack_next() {
    let act = Package::new().with_next(|pack| pack.with_id("pack2"));
    assert_eq!(act.next.unwrap().id, "pack2");
}
