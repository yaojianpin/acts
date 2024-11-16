use crate::{
    sch::Runtime,
    store::{Cond, Expr, StoreAdapter},
    utils::Id,
    Query, Result, TaskInfo,
};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct TaskExecutor {
    runtime: Arc<Runtime>,
}

impl TaskExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self))]
    pub fn list(&self, pid: &str, count: usize) -> Result<Vec<TaskInfo>> {
        let query = Query::new()
            .push(Cond::and().push(Expr::eq("pid", pid.to_string())))
            .set_limit(10000);
        match self.runtime.cache().store().tasks().query(&query) {
            Ok(mut tasks) => {
                let mut ret = Vec::new();
                tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                for t in tasks.into_iter().take(count) {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, pid: &str, tid: &str) -> Result<TaskInfo> {
        let id = Id::new(pid, tid);
        match self.runtime.cache().store().tasks().find(&id.id()) {
            Ok(t) => Ok(t.into()),
            Err(err) => Err(err),
        }
    }
}
