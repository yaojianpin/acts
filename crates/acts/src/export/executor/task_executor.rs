use crate::{Result, TaskInfo, query::Query, scheduler::Runtime, store::PageData, utils::Id};
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
    pub fn list(&self, q: &Query) -> Result<PageData<TaskInfo>> {
        match self.runtime.cache().store().tasks().query(q) {
            Ok(tasks) => Ok(PageData {
                count: tasks.count,
                page_size: tasks.page_size,
                page_count: tasks.page_count,
                page_num: tasks.page_num,
                rows: tasks.rows.iter().map(|m| m.into()).collect(),
            }),
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
