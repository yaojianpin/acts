mod act_executor;
mod event_executor;
mod message_executor;
mod model_executor;
mod package_executor;
mod process_executor;
mod task_executor;

use serde::Serialize;
use serde_json::json;

use crate::{scheduler::Runtime, store::query::*};
use std::sync::Arc;

#[derive(Default, Debug)]
pub struct ExecutorQuery {
    pub query_by: Vec<(String, serde_json::Value)>,
    pub order_by: Vec<(String, bool)>,

    pub offset: usize,
    pub count: usize,
}

#[derive(Clone)]
pub struct Executor {
    msg: message_executor::MessageExecutor,
    act: act_executor::ActExecutor,
    model: model_executor::ModelExecutor,
    proc: process_executor::ProcessExecutor,
    task: task_executor::TaskExecutor,
    pack: package_executor::PackageExecutor,
    evt: event_executor::EventExecutor,
}

impl ExecutorQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    pub fn with_count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    pub fn with_order(mut self, order: &str, rev: bool) -> Self {
        self.order_by.push((order.to_string(), rev));
        self
    }

    pub fn with_query<T: Serialize>(mut self, key: &str, value: T) -> Self {
        self.query_by.push((key.to_string(), json!(value)));
        self
    }

    pub fn into_cond(&self) -> Cond {
        let mut cond = Cond::and();
        for (k, v) in self.query_by.iter() {
            let mut key: &str = k;
            if k == "type" {
                key = "kind";
            }
            cond = cond.push(Expr::eq(key, v))
        }
        cond
    }

    pub fn into_query(&self) -> Query {
        let mut query = Query::new().set_offset(self.offset).set_limit(self.count);
        if !self.query_by.is_empty() {
            query = query.push(self.into_cond())
        }
        query.set_order(&self.order_by)
    }
}

impl Executor {
    pub(crate) fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            msg: message_executor::MessageExecutor::new(rt),
            act: act_executor::ActExecutor::new(rt),
            model: model_executor::ModelExecutor::new(rt),
            proc: process_executor::ProcessExecutor::new(rt),
            task: task_executor::TaskExecutor::new(rt),
            pack: package_executor::PackageExecutor::new(rt),
            evt: event_executor::EventExecutor::new(rt),
        }
    }

    /// executor for related message functions
    pub fn msg(&self) -> &message_executor::MessageExecutor {
        &self.msg
    }

    /// executor for related act operations
    /// such as 'complete', 'back', 'cancel' ..
    pub fn act(&self) -> &act_executor::ActExecutor {
        &self.act
    }

    /// executor for related model functions
    pub fn model(&self) -> &model_executor::ModelExecutor {
        &self.model
    }

    /// executor for related process functions
    pub fn proc(&self) -> &process_executor::ProcessExecutor {
        &self.proc
    }

    /// executor for related task functions
    pub fn task(&self) -> &task_executor::TaskExecutor {
        &self.task
    }

    /// executor for related package functions
    pub fn pack(&self) -> &package_executor::PackageExecutor {
        &self.pack
    }

    /// executor for related event functions
    pub fn evt(&self) -> &event_executor::EventExecutor {
        &self.evt
    }
}
