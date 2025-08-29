use std::collections::HashMap;

use acts::{Event, Executor, Message, MessageState, Result, Vars};
use serde_json::json;

type ReqAction = fn(&Executor, &Event<Message>) -> Result<()>;
type MsgAction = fn(&Executor, &Event<Message>);
pub struct Client {
    actions: HashMap<String, Box<ReqAction>>,
    messages: HashMap<String, Box<MsgAction>>,
}

impl Client {
    pub fn new() -> Self {
        let actions: HashMap<_, _> = [
            ("init".to_string(), Box::new(Self::init as ReqAction)),
            (
                "step1_timeout_5s".to_string(),
                Box::new(Self::timeout_5s as ReqAction),
            ),
        ]
        .into();

        let messages: HashMap<_, _> = [(
            "step1_timeout_2s".to_string(),
            Box::new(Self::timeout_2s as MsgAction),
        )]
        .into();

        Self { actions, messages }
    }

    pub fn process(&self, executor: &Executor, e: &Event<Message>) -> Result<()> {
        if e.is_irq() && e.is_state(MessageState::Created) {
            match self.actions.get(&e.key) {
                Some(action) => {
                    action(executor, e)?;
                    println!("action state: key={}", &e.key);
                }
                None => println!("'{}' is waitting for timeout", e.key),
            }
        }

        if e.is_msg()
            && let Some(action) = self.messages.get(&e.key)
        {
            action(executor, e);
        }

        Ok(())
    }

    pub fn init(executor: &Executor, e: &Event<Message>) -> Result<()> {
        println!("req: {} inputs={}", e.key, e.inputs);
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u1"));
        executor.act().complete(&e.pid, &e.tid, &vars)
    }

    pub fn timeout_2s(_executor: &Executor, e: &Event<Message>) {
        println!("msg: {} inputs={}", e.key, e.inputs);
    }
    pub fn timeout_5s(executor: &Executor, e: &Event<Message>) -> Result<()> {
        println!("req: {} inputs={}", e.key, e.inputs);
        executor.act().complete(&e.pid, &e.tid, &Vars::new())
    }
}
