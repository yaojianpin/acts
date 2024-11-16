use crate::{Act, Block, StmtBuild};

#[test]
fn model_act_block_parse() {
    let text = r#"
    act: block
    then:
      - act: msg
        key: msg1
         
    inputs:
      a: 5
    
    next: 
      id: pack2
    "#;
    if let Ok(Act {
        act,
        inputs,
        then,
        next,
        ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "block");
        assert_eq!(then.len(), 1);
        assert_eq!(inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(next.unwrap().id, "pack2");
    } else {
        assert!(false);
    }
}

// #[test]
// fn model_act_block_id() {
//     let act = Block::new().with_id("pack1");
//     assert_eq!(act.id, "pack1");
// }

#[test]
fn model_act_block_input() {
    let act = Block::new().with_input("a", 5);
    assert_eq!(act.inputs.get::<i32>("a").unwrap(), 5);
}

#[test]
fn model_act_block_acts() {
    let act = Block::new().with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg1"))));
    assert_eq!(act.then.len(), 1);
}

#[test]
fn model_act_pack_next() {
    let act = Block::new().with_next(|act| act.with_id("pack2"));
    assert_eq!(act.next.unwrap().id, "pack2");
}
