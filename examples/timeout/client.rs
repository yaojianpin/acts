use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use acts::{Event, Executor, Message, MessageState, Result, Vars};
use serde_json::json;

type ReqActionFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type ReqAction = fn(&Executor, &Event<Message>) -> ReqActionFuture;
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

    pub async fn process(&self, executor: &Executor, e: &Event<Message>) -> Result<()> {
        if let Some(params) = e.params()
            && let Some(key) = params.get::<String>("key")
        {
            if e.is_irq() && e.is_state(MessageState::Created) {
                match self.actions.get(&key) {
                    Some(action) => {
                        action(executor, e).await?;
                        println!("action state: key={}", key);
                    }
                    None => println!("'{}' is waitting for timeout", key),
                }
            }

            if e.is_msg()
                && let Some(action) = self.messages.get(&key)
            {
                action(executor, e);
            }
        }

        Ok(())
    }

    pub fn init(executor: &Executor, e: &Event<Message>) -> ReqActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        println!("req: {} inputs={}", pid, e.inputs);
        Box::pin(async move {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            executor.act().complete(&pid, &tid, vars).await
        })
    }

    pub fn timeout_2s(_executor: &Executor, e: &Event<Message>) {
        println!("msg: {} inputs={}", e.name, e.inputs);
    }
    pub fn timeout_5s(executor: &Executor, e: &Event<Message>) -> ReqActionFuture {
        let executor = executor.clone();
        let pid = e.pid.clone();
        let tid = e.tid.clone();
        Box::pin(async move { executor.act().complete(&pid, &tid, Vars::new()).await })
    }
}
