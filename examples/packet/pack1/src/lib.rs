#[allow(warnings)]
mod bindings;

use bindings::acts::packs::{act, log};
use bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        let inputs = act::inputs();
        log::info(&format!("inputs={:?}", inputs));

        act::set_data("a", &act::Value::PosInt(100));
        act::set_output("b", &act::Value::Text(String::from("abc")));

        // act::push(&Packet::Req(Request {
        //     id: "test1".to_string(),
        //     key: Some("key1".to_string()),
        //     tag: Some("tag1".to_string()),
        //     name: Some("name1".to_string()),
        //     inputs: Map::new(),
        //     outputs: Map::new(),
        // }));
    }
}

bindings::export!(Component with_types_in bindings);
