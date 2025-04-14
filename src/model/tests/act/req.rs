use crate::{Act, Irq};
use serde_json::json;

#[test]
fn model_act_parse_req() {
    let text = r#"
    act: irq
    tag: tag1
    key: msg1
    inputs:
        a: 1
    rets:
        b:
    "#;
    if let Ok(Act {
        act,
        key,
        tag,
        inputs,
        rets,
        ..
    }) = serde_yaml::from_str(text)
    {
        assert_eq!(act, "irq");
        assert_eq!(key, "msg1");
        assert_eq!(tag, "tag1");
        assert_eq!(inputs.get::<i32>("a").unwrap(), 1);
        assert_eq!(rets.get_value("b").unwrap(), &json!(null));
    } else {
        panic!();
    }
}

#[test]
fn model_act_req_with() {
    let act = Irq::new().with_input("p1", 5);
    assert_eq!(act.inputs.len(), 1);
    assert_eq!(act.inputs.get::<i32>("p1"), Some(5));
}

// #[test]
// fn model_act_req_outputs() {
//     let act = Req::new().with_output("p1", 5);
//     assert_eq!(act.outputs.len(), 1);
//     assert!(act.outputs.get_value("p1").is_some());
// }

#[test]
fn model_act_req_tag() {
    let act = Irq::new().with_tag("tag1");
    assert_eq!(act.tag, "tag1");
}

#[test]
fn model_act_req_key() {
    let act = Irq::new().with_key("key1");
    assert_eq!(act.key, "key1");
}

// #[test]
// fn model_act_req_catch() {
//     let act = Req::new().with_catch(|c| c.with_err("err1"));
//     assert_eq!(act.catches.len(), 1);
// }

// #[test]
// fn model_act_req_timeout() {
//     let act = Req::new().with_timeout(|c| c.with_on(r#"1d"#));
//     assert_eq!(act.timeout.len(), 1);
// }

// #[test]
// fn model_act_req_on_created() {
//     let act = Req::new().with_on_created(|acts| acts.add(ActFn::set(Vars::new().with("a", 5))));
//     assert_eq!(act.on_created.len(), 1);
// }

// #[test]
// fn model_act_req_on_completed() {
//     let act = Req::new().with_on_completed(|acts| acts.add(ActFn::set(Vars::new().with("a", 5))));
//     assert_eq!(act.on_completed.len(), 1);
// }
