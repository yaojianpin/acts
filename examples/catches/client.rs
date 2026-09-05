use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use acts::{Event, Executor, Message, MessageState, Result, Vars};
use serde_json::json;

type ActionFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Action = fn(&Executor, &Event<Message>) -> ActionFuture;
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

    pub async fn process(&self, executor: &Executor, e: &Event<Message>) -> Result<()> {
        if e.is_irq() && e.is_state(MessageState::Created) {
            let key = e.params().unwrap().get::<String>("key").unwrap();
            match self.actions.get(&key) {
                Some(action) => {
                    println!("action:{} inputs={:?}", key, e.inputs);
                    action(executor, e).await?;
                }
                None => eprintln!("cannot find action '{}'", key),
            }
        }

        Ok(())
    }

    pub fn init(executor: &Executor, e: &Event<Message>) -> ActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        Box::pin(async move {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            executor.act().complete(&pid, &tid, vars).await
        })
    }
    pub fn act1(executor: &Executor, e: &Event<Message>) -> ActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        Box::pin(async move {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u2"));

            // will catch by catch1
            vars.set("ecode", "err1");

            // cause the error
            executor.act().fail(&pid, &tid, vars).await
        })
    }
    pub fn catch1(executor: &Executor, e: &Event<Message>) -> ActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        Box::pin(async move {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u3"));
            vars.set("ecode", "err1");

            executor.act().complete(&pid, &tid, vars).await
        })
    }
    pub fn catch2(executor: &Executor, e: &Event<Message>) -> ActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        Box::pin(async move {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u4"));

            executor.act().complete(&pid, &tid, vars).await
        })
    }

    pub fn catch_others(executor: &Executor, e: &Event<Message>) -> ActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        Box::pin(async move {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u5"));
            executor.act().complete(&pid, &tid, vars).await
        })
    }
}
