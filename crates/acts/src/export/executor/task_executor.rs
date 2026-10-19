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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, data::TaskVars};

    fn info() -> TaskInfo {
        TaskInfo {
            id: "t1".to_string(),
            prev: None,
            name: String::new(),
            pid: "p1".to_string(),
            nid: String::new(),
            r#type: String::new(),
            state: String::new(),
            data: String::new(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
        }
    }

    /// A vars row with no data leaves the lifecycle row as it was; a missing
    /// vars row does too.
    #[tokio::test]
    async fn join_vars_keeps_the_lifecycle_row_when_there_is_nothing_to_join() {
        let engine = Engine::builder().start().await.unwrap();
        let store = engine.runtime().store();
        let mut info = info();
        let executor = TaskExecutor::new(&store, &Arc::new(Principal::unrestricted()));

        // no vars row at all
        executor.join_vars(&mut info).await;
        assert!(info.data.is_empty());

        // a vars row whose data is empty
        store
            .vars()
            .create(&TaskVars {
                id: Id::new(&info.pid, &info.id).id(),
                pid: info.pid.clone(),
                tid: info.id.clone(),
                data: String::new(),
                sealed: String::new(),
                source_revision: String::new(),
                v: 0,
            })
            .await
            .unwrap();
        executor.join_vars(&mut info).await;
        assert!(info.data.is_empty());
    }

    /// A vars row with data is joined onto the row.
    #[tokio::test]
    async fn join_vars_fills_the_scope_data() {
        let engine = Engine::builder().start().await.unwrap();
        let store = engine.runtime().store();
        let mut info = info();
        store
            .vars()
            .create(&TaskVars {
                id: Id::new(&info.pid, &info.id).id(),
                pid: info.pid.clone(),
                tid: info.id.clone(),
                data: r#"{"a":1}"#.to_string(),
                sealed: String::new(),
                source_revision: String::new(),
                v: 0,
            })
            .await
            .unwrap();
        let executor = TaskExecutor::new(&store, &Arc::new(Principal::unrestricted()));
        executor.join_vars(&mut info).await;
        assert_eq!(info.data, r#"{"a":1}"#);
    }
}
