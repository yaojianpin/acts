use acts::{Executor, Message, MessageState, Result, Vars};
use serde_json::json;
use std::collections::HashMap;

type Action = fn(&Vars) -> Vars;
pub struct Client {
    actions: HashMap<String, Box<Action>>,
}

impl Client {
    pub fn new() -> Self {
        let actions: HashMap<_, _> = [
            ("init".to_string(), Box::new(Self::init as Action)),
            ("action1".to_string(), Box::new(Self::action1)),
            ("action2".to_string(), Box::new(Self::action2)),
        ]
        .into();

        Self { actions }
    }

    pub fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        println!("process: {:?}", message);
        if message.is_type("act") && message.is_state(MessageState::Created) {
            match self.actions.get(&message.key) {
                Some(action) => {
                    let outputs = action(&message.inputs);
                    executor
                        .act()
                        .complete(&message.pid, &message.tid, &outputs)?;
                    println!("action state: key={}", &message.key);
                    println!("inputs:{:?}", &message.inputs);
                    println!("outputs:{:?}", &outputs);
                    println!();
                }
                None => eprintln!("cannot find action '{}'", message.key),
            }
        }

        Ok(())
    }

    pub fn init(_inputs: &Vars) -> Vars {
        println!("init");
        let mut vars = Vars::new();
        vars.insert("v".to_string(), json!(10));
        vars
    }
    pub fn action1(_inputs: &Vars) -> Vars {
        println!("action1");
        let mut vars = Vars::new();
        vars.insert("v".to_string(), json!(100));

        vars
    }
    pub fn action2(inputs: &Vars) -> Vars {
        println!("action2: {inputs}");
        let v = inputs.get_value("v").unwrap().as_i64().unwrap();

        let mut vars = Vars::new();
        vars.insert("v".to_string(), json!(v * 2));

        vars
    }
}
