use std::collections::HashMap;

use acts::{Executor, Message, Result, Vars};
use serde_json::json;

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
            ("action3".to_string(), Box::new(Self::action3)),
            ("action4".to_string(), Box::new(Self::action4)),
        ]
        .into();

        Self { actions }
    }

    pub fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        if message.is_source("act") && message.is_state("created") {
            match self.actions.get(&message.key) {
                Some(action) => {
                    let outputs = action(&message.inputs);
                    executor.complete(&message.pid, &message.tid, &outputs)?;
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
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u1"));

        // set the condition var
        vars.insert("v".to_string(), json!(10));
        vars
    }
    pub fn action1(_inputs: &Vars) -> Vars {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u2"));

        // change the result var
        vars.insert("v".to_string(), json!(100));

        vars
    }
    pub fn action2(inputs: &Vars) -> Vars {
        let result = inputs.get_value("v").unwrap().as_i64().unwrap();

        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u3"));

        // change the result var again
        vars.insert("v".to_string(), json!(result * 2));

        vars
    }
    pub fn action3(_inputs: &Vars) -> Vars {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u4"));

        vars
    }

    pub fn action4(inputs: &Vars) -> Vars {
        println!("result:{:?}", inputs.get_value("v"));

        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u5"));

        vars
    }
}
