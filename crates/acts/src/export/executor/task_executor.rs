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

    /// Join a task's scope vars row onto a durable lifecycle row — the vars
    /// collection holds the task's own data, keyed by the same composite id.
    async fn join_vars(&self, info: &mut TaskInfo) {
        let Ok(vars) = self
            .runtime
            .cache()
            .store()
            .vars()
            .find(&Id::new(&info.pid, &info.id).id())
            .await
        else {
            return;
        };
        if !vars.data.is_empty() {
            info.data = vars.data;
        }
    }

    #[instrument(skip(self))]
    pub async fn list(&self, q: &Query) -> Result<PageData<TaskInfo>> {
        match self.runtime.cache().store().tasks().query(q).await {
            Ok(tasks) => {
                let mut rows: Vec<TaskInfo> = tasks.rows.iter().map(|m| m.into()).collect();
                for row in rows.iter_mut() {
                    self.join_vars(row).await;
                }
                Ok(PageData {
                    count: tasks.count,
                    page_size: tasks.page_size,
                    page_count: tasks.page_count,
                    page_num: tasks.page_num,
                    rows,
                })
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub async fn get(&self, pid: &str, tid: &str) -> Result<TaskInfo> {
        let id = Id::new(pid, tid);
        match self.runtime.cache().store().tasks().find(&id.id()).await {
            Ok(t) => {
                let mut info: TaskInfo = t.into();
                self.join_vars(&mut info).await;
                Ok(info)
            }
            Err(err) => Err(err),
        }
    }
}
