use std::collections::HashMap;

use acts::{ActionResult, Event, Executor, Message, Result, Vars};
use serde_json::json;

type ReqAction = fn(&Executor, &Event<Message>) -> Result<ActionResult>;
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
        if e.is_type("req") && e.is_state("created") {
            match self.actions.get(&e.key) {
                Some(action) => {
                    let state = action(executor, e)?;
                    println!("action state: key={} cost={}ms", &e.key, state.cost(),);
                }
                None => println!("'{}' is waitting for timeout", e.key),
            }
        }

        if e.is_type("msg") {
            match self.messages.get(&e.key) {
                Some(action) => {
                    action(executor, e);
                }
                None => {}
            }
        }

        Ok(())
    }

    pub fn init(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        println!("req: {} inputs={}", e.key, e.inputs);
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u1"));
        executor.complete(&e.proc_id, &e.id, &vars)
    }

    pub fn timeout_2s(_executor: &Executor, e: &Event<Message>) {
        println!("msg: {} inputs={}", e.key, e.inputs);
    }
    pub fn timeout_5s(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        println!("req: {} inputs={}", e.key, e.inputs);
        executor.complete(&e.proc_id, &e.id, &Vars::new())
    }
}
