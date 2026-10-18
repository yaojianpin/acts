//! The `Store` methods that materialize the read model.
//!
//! Two groups live here: the process/task rows a process is rebuilt from
//! ([`Store::load_proc`], the read API behind `proc:get` and `Runtime::proc`)
//! and the delivery rows the channel path settles. Both are written by the
//! kernel's row projection; nothing here drives a run.
use crate::{
    ActError, Error, Result, Vars, Workflow,
    scheduler::{LegacyTaskState, NodeData, Runtime},
    store::{Store, query::*},
    utils,
};
use std::sync::Arc;
use tracing::debug;

impl Store {
    /// Rebuild one process from its durable rows: the proc row, every task row
    /// of the process (each task's node comes from its own persisted
    /// `node_data`), and each task's vars row. `None` when the process has no
    /// row — a settled run whose rows the sweeper already reclaimed.
    pub async fn load_proc(
        &self,
        pid: &str,
        rt: &Arc<Runtime>,
    ) -> Result<Option<Arc<crate::scheduler::Process>>> {
        debug!("load process pid={}", pid);
        let Some(p) = self.procs().find_opt(pid).await? else {
            return Ok(None);
        };
        let model = Workflow::from_json(&p.model)?;
        let proc = crate::scheduler::Process::new(pid);
        let env_local: serde_json::Value =
            serde_json::from_str(&p.env).map_err(|err| ActError::Store(err.to_string()))?;

        proc.load_owned(model)?;
        proc.set_pure_state(p.state.into());
        proc.set_start_time(p.start_time);
        proc.set_env(&env_local.into());
        self.load_tasks(&proc, rt).await?;
        if let Some(err) = p.err {
            let err: Error =
                serde_json::from_str(&err).map_err(|err| ActError::Store(err.to_string()))?;
            proc.set_pure_err(&err)
        }
        Ok(Some(proc))
    }

    pub async fn remove_proc(&self, pid: &str) -> Result<bool> {
        debug!("remove_proc pid={}", pid);
        // All rows of the process — tasks, outbox ops, its message/delivery
        // rows and the proc row — are removed as ONE atomic batch, so a crash
        // mid-removal cannot leave a half-deleted process behind nor orphaned
        // message/delivery rows that would be retried forever.
        self.remove_proc_rows(pid).await
    }

    /// The directory a process's filesystem access was confined to, read from
    /// its durable row: the process env carries it under a private key, so it
    /// is still reachable once the in-memory instance is gone — the sweeper
    /// removes a *finished* process's directory, and a start that never became
    /// durable removes its own. `None` when there is no row, the process was
    /// started without an ACL workdir, or the stored env cannot be read: the
    /// directory is reclaimed best-effort and a row that cannot answer must
    /// never be a reason to keep the process's rows alive.
    pub(crate) async fn proc_workdir(&self, pid: &str) -> Option<std::path::PathBuf> {
        let proc = self.procs().find_opt(pid).await.ok()??;
        let env: serde_json::Value = serde_json::from_str(&proc.env).ok()?;
        Vars::from(env).get::<std::path::PathBuf>(utils::consts::PROC_WORKDIR)
    }

    /// Rebuild the process's tasks from their rows (`load_proc`'s second
    /// half).
    async fn load_tasks(
        &self,
        proc: &Arc<crate::scheduler::Process>,
        rt: &Arc<Runtime>,
    ) -> Result<()> {
        debug!("load_tasks pid={}", proc.id());
        let collection = self.tasks();
        let query = Query::new().filter(Filter::and().expr(Expr::eq("pid", proc.id())));
        let tasks = collection.query_all(&query).await?;

        // The row carries the node's persisted shape, so a task is rebuilt
        // from its own `node_data` — no model lookup and no live node graph.
        for t in tasks {
            let node: NodeData = serde_json::from_str(&t.node_data)
                .map_err(|err| ActError::Store(err.to_string()))?;
            let state: LegacyTaskState = t.state.into();
            let mut task = crate::scheduler::Task::new(proc, &t.tid, node, rt);
            task.set_pure_state(state.clone());
            task.set_start_time(t.start_time);
            task.set_end_time(t.end_time);
            task.timestamp = t.timestamp;
            if let Some(prev) = &t.prev {
                task.set_prev(prev);
            }

            if let Some(parent) = &t.parent {
                task.set_parent(parent);
            }

            // resume next tasks
            for next in t.next.iter() {
                task.set_next(next);
            }

            if let Some(err) = t.err {
                let err: Error =
                    serde_json::from_str(&err).map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_err(&err)
            }
            proc.push_task(Arc::new(task))?;
        }

        // attach each task scope's persisted vars (its own data and
        // sealed rows) onto the restored tasks — scope vars live in the vars
        // collection, keyed by the same composite id as the lifecycle row
        let vars = self.vars();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", proc.id())));
        for row in vars.query_all(&q).await? {
            let Some(task) = proc.task(&row.tid) else {
                continue;
            };
            if !row.data.is_empty() {
                let data = serde_json::from_str(&row.data)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_data(&data);
            }
            if !row.sealed.is_empty() {
                let data = serde_json::from_str(&row.sealed)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_sealed_data(&data);
            }
        }

        Ok(())
    }
}
