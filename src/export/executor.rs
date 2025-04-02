mod act;
mod model;
mod msg;
mod pack;
mod proc;
mod task;

use crate::{
  scheduler::Runtime,
  store::{Cond, Expr},
  Query,
};
use std::sync::Arc;

#[derive(Default, Debug)]
pub struct ExecutorQuery {
    pub query_by: Vec<(String, String)>,
    pub order_by: Vec<(String, bool)>,

    pub offset: usize,
    pub count: usize,
}

#[derive(Clone)]
pub struct Executor {
    msg: msg::MessageExecutor,
    act: act::ActExecutor,
    model: model::ModelExecutor,
    proc: proc::ProcExecutor,
    task: task::TaskExecutor,
    pack: pack::PackageExecutor,
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

    pub fn with_query(mut self, key: &str, value: &str) -> Self {
        self.query_by.push((key.to_string(), value.to_string()));
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
            msg: msg::MessageExecutor::new(rt),
            act: act::ActExecutor::new(rt),
            model: model::ModelExecutor::new(rt),
            proc: proc::ProcExecutor::new(rt),
            task: task::TaskExecutor::new(rt),
            pack: pack::PackageExecutor::new(rt),
        }
    }

    /// executor for related message functions
    pub fn msg(&self) -> &msg::MessageExecutor {
        &self.msg
    }

    /// executor for related act operations
    /// such as 'complete', 'back', 'cancel' ..
    pub fn act(&self) -> &act::ActExecutor {
        &self.act
    }

    /// executor for related model functions
    pub fn model(&self) -> &model::ModelExecutor {
        &self.model
    }

    /// executor for related proc functions
    pub fn proc(&self) -> &proc::ProcExecutor {
        &self.proc
    }

    /// executor for related task functions
    pub fn task(&self) -> &task::TaskExecutor {
        &self.task
    }

    /// executor for related package functions
    pub fn pack(&self) -> &pack::PackageExecutor {
        &self.pack
    }
}
