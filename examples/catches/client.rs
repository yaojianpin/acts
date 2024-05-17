use std::collections::HashMap;

use acts::{ActionResult, Event, Executor, Message, Result, Vars};
use serde_json::json;

type Action = fn(&Executor, &Event<Message>) -> Result<ActionResult>;
pub struct Client {
    actions: HashMap<String, Box<Action>>,
}

impl Client {
    pub fn new() -> Self {
        let actions: HashMap<_, _> = [
            ("init".to_string(), Box::new(Self::init as Action)),
            ("act1".to_string(), Box::new(Self::act1)),
            ("catch1".to_string(), Box::new(Self::catch1)),
            ("catch2".to_string(), Box::new(Self::catch2)),
            ("catch_others".to_string(), Box::new(Self::catch_others)),
        ]
        .into();

        Self { actions }
    }

    pub fn process(&self, executor: &Executor, e: &Event<Message>) -> Result<()> {
        if e.is_source("act") && e.is_state("created") {
            match self.actions.get(&e.key) {
                Some(action) => {
                    let state = action(executor, e)?;
                    println!("action state: key={} cost={}ms", &e.key, state.cost(),);
                }
                None => eprintln!("cannot find action '{}'", e.key),
            }
        }

        Ok(())
    }

    pub fn init(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u1"));
        executor.complete(&e.pid, &e.tid, &vars)
    }
    pub fn act1(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u2"));

        // will catch by catch1
        vars.insert("error".to_string(), json!({ "ecode": "err1" }));

        // will catch by catch2
        // vars.insert("error".to_string(), json!({ "ecode": "err2" }));

        // will catch by catch_others
        // vars.insert("error".to_string(), json!({ "ecode": "the_other_err_code" }));

        // cause the error
        executor.error(&e.pid, &e.tid, &vars)
    }
    pub fn catch1(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u3"));
        vars.insert("error".to_string(), json!({ "ecode":"err1"}));

        executor.complete(&e.pid, &e.tid, &vars)
    }
    pub fn catch2(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u4"));

        executor.complete(&e.pid, &e.tid, &vars)
    }

    pub fn catch_others(executor: &Executor, e: &Event<Message>) -> Result<ActionResult> {
        let mut vars = Vars::new();
        vars.insert("uid".to_string(), json!("u5"));
        executor.complete(&e.pid, &e.tid, &vars)
    }
}
