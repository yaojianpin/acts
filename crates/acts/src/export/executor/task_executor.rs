use crate::{Principal, Result, TaskInfo, query::Query, store::PageData, store::Store, utils::Id};
use std::sync::Arc;
use tracing::instrument;

/// `task:ls` — list task rows.
pub(crate) const LS: &str = "task:ls";
/// `task:get` — read one task row, with its scope vars.
pub(crate) const GET: &str = "task:get";

#[derive(Clone)]
pub struct TaskExecutor {
    store: Arc<Store>,
    principal: Arc<Principal>,
}

impl TaskExecutor {
    pub(crate) fn new(store: &Arc<Store>, principal: &Arc<Principal>) -> Self {
        Self {
            store: store.clone(),
            principal: principal.clone(),
        }
    }

    /// Join a task's scope vars row onto a durable lifecycle row — the vars
    /// collection holds the task's own data, keyed by the same composite id.
    async fn join_vars(&self, info: &mut TaskInfo) {
        let Ok(vars) = self
            .store
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
        self.principal.check(LS)?;
        let tasks = self.store.clone().tasks().query(q).await?;
        let mut rows: Vec<TaskInfo> = tasks.rows.iter().map(|m| m.into()).collect();
        for row in rows.iter_mut() {
            self.join_vars(row).await;
        }
        Ok(tasks.with_rows(rows))
    }

    #[instrument(skip(self))]
    pub async fn get(&self, pid: &str, tid: &str) -> Result<TaskInfo> {
        self.principal.check(GET)?;
        let id = Id::new(pid, tid);
        match self.store.clone().tasks().find(&id.id()).await {
            Ok(t) => {
                let mut info: TaskInfo = t.into();
                self.join_vars(&mut info).await;
                Ok(info)
            }
            Err(err) => Err(err),
        }
    }
}
