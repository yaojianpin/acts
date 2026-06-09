use acts::{ActError, Executor, Message, MessageState, Result, Vars};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

type Action = fn(&Parmas) -> Vars;
pub struct Client {
    actions: HashMap<String, Box<Action>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Parmas {
    key: String,
    v: i64,
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

    pub async fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        println!("process: {message:?}");
        if message.is_type("act") && message.is_state(MessageState::Created) {
            let params = message
                .inputs
                .get::<Parmas>("params")
                .ok_or(ActError::Action("message.params is null".to_string()))?;
            match self.actions.get(&params.key) {
                Some(action) => {
                    let outputs = action(&params);
                    executor
                        .act()
                        .complete(&message.pid, &message.tid, outputs.clone())?;
                    println!("action state: key={}", &params.key);
                    println!("inputs:{:?}", &message.inputs);
                    println!("outputs:{outputs:?}");
                    println!();
                }
                None => eprintln!("cannot find action '{}'", params.key),
            }
        }

        Ok(())
    }

    fn init(_params: &Parmas) -> Vars {
        println!("init");
        let mut vars = Vars::new();
        vars.insert("v".to_string(), json!(10));
        vars
    }
    fn action1(_params: &Parmas) -> Vars {
        println!("action1");
        let mut vars = Vars::new();
        vars.insert("v".to_string(), json!(100));

        vars
    }
    fn action2(params: &Parmas) -> Vars {
        println!("action2: {params:?}");

        let mut vars = Vars::new();
        vars.insert("v".to_string(), json!(params.v * 2));

        vars
    }
}
