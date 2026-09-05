use acts::{ActError, Executor, Message, MessageState, Result, Vars};
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

    pub async fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        // println!("process: {message:?}");
        if message.is_type("act") && message.is_state(MessageState::Created) {
            let params = message
                .params()
                .ok_or(ActError::Action("message.params is null".to_string()))?;
            let key = params.get::<String>("key").unwrap_or_default();
            match self.actions.get(&key) {
                Some(action) => {
                    let outputs = action(&params);
                    executor
                        .act()
                        .complete(&message.pid, &message.tid, outputs.clone())
                        .await?;
                    println!("action state: key={}", key);
                    println!("inputs:{:?}", message.inputs);
                    println!("outputs:{outputs:?}");
                    println!();
                }
                None => eprintln!("cannot find action '{}'", key),
            }
        }

        Ok(())
    }

    fn init(_params: &Vars) -> Vars {
        println!("init");
        let mut vars = Vars::new();
        vars.set("v", 10);
        vars
    }
    fn action1(_params: &Vars) -> Vars {
        println!("action1");
        let mut vars = Vars::new();
        vars.set("v", 100);

        vars
    }
    fn action2(params: &Vars) -> Vars {
        println!("action2: {params:?}");
        let v = params.get::<i64>("v").unwrap();
        let mut vars = Vars::new();
        vars.set("v", v * 2);

        vars
    }
}
