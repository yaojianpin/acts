//! The row-based read model behind `proc:get`.
//!
//! A process is read straight from its durable rows — the proc row, its task
//! rows and each task's vars row — instead of being rebuilt into a resident
//! process/task object. The rows are the aggregate's projection; nothing here
//! drives a run, and no snapshot is loaded.
//!
//! `proc:get` reads the full surface; the accessors beyond it are the crate's
//! read-model test surface (the reliability matrix and the env/export tests).
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;

use crate::{
    ActError, Error, Result, Vars, Workflow,
    scheduler::{NodeData, core::model::task_graph::TaskState},
    store::{
        Store, data,
        query::{Expr, Filter, Query},
    },
    utils::consts,
};

/// One task's projected rows: the lifecycle row, the node it names (the
/// persisted `node_data` shape) and its joined vars row.
#[derive(Clone, Debug)]
pub(crate) struct TaskRow {
    row: data::Task,
    node: NodeData,
    /// The row's state; `None` for `"none"`/an unknown word (a row written by
    /// another version).
    state: Option<TaskState>,
    err: Option<Error>,
    data: Vars,
}

impl TaskRow {
    /// The external instance id (`tid`): what messages carry and client
    /// actions address.
    pub(crate) fn tid(&self) -> &str {
        &self.row.tid
    }

    pub(crate) fn node(&self) -> &NodeData {
        &self.node
    }

    pub(crate) fn state(&self) -> Option<TaskState> {
        self.state
    }

    pub(crate) fn err(&self) -> Option<Error> {
        self.err.clone()
    }

    /// The task's own scope vars (the paired vars row), empty when the row
    /// carries none.
    pub(crate) fn data(&self) -> &Vars {
        &self.data
    }

    pub(crate) fn row(&self) -> &data::Task {
        &self.row
    }
}

/// One process's projected rows.
#[derive(Clone, Debug)]
pub(crate) struct ProcRead {
    proc: data::Proc,
    model: Workflow,
    env: Vars,
    tasks: Vec<TaskRow>,
}

impl ProcRead {
    pub(crate) fn id(&self) -> &str {
        &self.proc.id
    }

    pub(crate) fn state(&self) -> Option<TaskState> {
        TaskState::parse(self.proc.state.as_str())
    }

    /// The durable model the rows were projected from.
    pub(crate) fn model(&self) -> &Workflow {
        &self.model
    }

    /// The run's env, as the proc row carries it (owner authority, directory
    /// and the durable `__env` map).
    pub(crate) fn env(&self) -> &Vars {
        &self.env
    }

    /// The tasks in the legacy read order: declaration map (`tid`) first, then
    /// start time — the order `proc:get` has always answered in.
    pub(crate) fn tasks(&self) -> &[TaskRow] {
        &self.tasks
    }

    pub(crate) fn task(&self, tid: &str) -> Option<&TaskRow> {
        self.tasks.iter().find(|task| task.row.tid == tid)
    }

    /// Every task of the model node `nid`, matching the legacy
    /// `Process::task_by_nid` (node id or declared content id).
    pub(crate) fn task_by_nid(&self, nid: &str) -> Vec<TaskRow> {
        self.tasks
            .iter()
            .filter(|task| task.node.id() == nid || task.node.content.id() == nid)
            .cloned()
            .collect()
    }

    pub(crate) fn owner_scope(&self) -> crate::ScopePolicy {
        self.env.get(consts::PROC_OWNER).unwrap_or_default()
    }

    pub(crate) fn workdir(&self) -> Option<std::path::PathBuf> {
        self.env.get(consts::PROC_WORKDIR)
    }
}

/// Load one process's projected rows. `None` when the process has no proc
/// row — a settled run whose rows the sweeper already reclaimed.
pub(crate) async fn load(store: &Store, pid: &str) -> Result<Option<ProcRead>> {
    let Some(proc) = store.procs().find_opt(pid).await? else {
        return Ok(None);
    };
    let model = Workflow::from_json(&proc.model)?;
    let env = match serde_json::from_str::<serde_json::Value>(&proc.env) {
        Ok(value) => Vars::from(value),
        Err(err) => return Err(ActError::Store(err.to_string())),
    };

    let query = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid)));
    let mut rows = store.tasks().query_all(&query).await?;
    // A task's vars live in its own row, joined by the composite id's `tid`.
    let mut vars: HashMap<String, data::TaskVars> = HashMap::new();
    for row in store.vars().query_all(&query).await? {
        vars.insert(row.tid.clone(), row);
    }

    // The legacy read model was a `BTreeMap` by tid, then a stable sort by
    // timestamp: keep that exact order so `proc:get`'s task list is unchanged.
    rows.sort_by(|a, b| a.tid.cmp(&b.tid));
    rows.sort_by_key(|row| row.timestamp);

    let mut tasks = Vec::with_capacity(rows.len());
    for row in rows {
        let node: NodeData =
            serde_json::from_str(&row.node_data).map_err(|err| ActError::Store(err.to_string()))?;
        let state = TaskState::parse(row.state.as_str());
        let err = match &row.err {
            Some(raw) => Some(
                serde_json::from_str::<Error>(raw)
                    .map_err(|err| ActError::Store(err.to_string()))?,
            ),
            None => None,
        };
        let data = match vars.get(&row.tid) {
            Some(vars) => parse_vars(&vars.data)?,
            None => Vars::new(),
        };
        tasks.push(TaskRow {
            row,
            node,
            state,
            err,
            data,
        });
    }

    Ok(Some(ProcRead {
        proc,
        model,
        env,
        tasks,
    }))
}

/// Build the headless context of a run's root task from its projected rows —
/// the read-model tests' replacement for the retired resident process/task
/// pair.
#[cfg(test)]
pub(crate) fn context(
    runtime: &std::sync::Arc<crate::scheduler::Runtime>,
    read: &ProcRead,
) -> Result<crate::scheduler::Context> {
    let root = read
        .tasks()
        .iter()
        .find(|task| task.node().kind() == crate::NodeKind::Workflow)
        .ok_or_else(|| ActError::Store("the run has no root task row".to_string()))?;
    let vars = root
        .data()
        .iter()
        .map(|(name, value)| (name.clone(), value.to_string()))
        .collect();
    let env = read
        .env()
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let spec = crate::scheduler::ScopeSpec {
        process_id: read.id().to_string(),
        node: root.node().id().to_string(),
        task_id: crate::scheduler::core::model::task_graph::TaskId::new(consts::TASK_ROOT_TID, 0)
            .map_err(|err| ActError::Runtime(err.to_string()))?,
        model: read.model().clone(),
        pushed: None,
        vars,
        snapshot: None,
        env,
        owner: read.owner_scope(),
    };
    let (_scope, _task, ctx) = crate::scheduler::headless_scope(runtime, &spec)?;
    Ok(ctx)
}

fn parse_vars(raw: &str) -> Result<Vars> {
    if raw.is_empty() {
        return Ok(Vars::new());
    }
    serde_json::from_str(raw).map_err(|err| ActError::Store(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Workflow;
    use crate::scheduler::core::model::transition_plan::ProcessVersion;
    use crate::store::MemoryStore;

    fn proc_row(model: &str, env: &str) -> data::Proc {
        data::Proc {
            id: "p-read".into(),
            state: "running".into(),
            mid: String::new(),
            name: String::new(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.into(),
            env: env.into(),
            err: None,
            removable: false,
            source_revision: ProcessVersion {
                graph: 1,
                process: 1,
                operations: 1,
            }
            .encode(),
            v: 1,
        }
    }

    /// An empty `vars.data` is the empty variable map; malformed JSON is a
    /// store error, never a panic.
    #[test]
    fn vars_text_is_parsed_strictly_but_empty_is_empty() {
        assert!(parse_vars("").unwrap().is_empty());
        assert_eq!(parse_vars(r#"{"a":1}"#).unwrap().get::<i32>("a"), Some(1));
        let error = parse_vars("[not json").unwrap_err();
        assert!(matches!(error, ActError::Store(_)), "{error:?}");
    }

    /// A proc row whose env cannot decode is a store error, not a run with no
    /// env.
    #[tokio::test]
    async fn an_unreadable_env_row_is_a_store_error() {
        let store = Store::new(std::sync::Arc::new(MemoryStore::new()));
        let model = serde_json::to_string(
            &Workflow::new()
                .with_id("wf-read")
                .with_step(|step| step.with_id("s1")),
        )
        .unwrap();
        assert!(
            store
                .procs()
                .create(&proc_row(&model, "not-json"))
                .await
                .unwrap()
        );
        let error = load(&store, "p-read").await.unwrap_err();
        assert!(matches!(error, ActError::Store(_)), "{error:?}");
    }

    /// A model row that cannot decode is a store error as well.
    #[tokio::test]
    async fn an_unreadable_model_row_is_a_store_error() {
        let store = Store::new(std::sync::Arc::new(MemoryStore::new()));
        assert!(
            store
                .procs()
                .create(&proc_row("not-json", "{}"))
                .await
                .unwrap()
        );
        let error = load(&store, "p-read").await.unwrap_err();
        assert!(matches!(error, ActError::Store(_)), "{error:?}");
    }

    /// A missing proc row is `None`: a reclaimed run has no reading.
    #[tokio::test]
    async fn a_reclaimed_run_reads_as_absent() {
        let store = Store::new(std::sync::Arc::new(MemoryStore::new()));
        assert!(load(&store, "p-missing").await.unwrap().is_none());
    }
}
