//! The headless scope both evaluation seams run against.
//!
//! A package effect and a branch predicate are evaluated the way the legacy
//! engine evaluated them: on a *real* task of the declaring model, with the
//! acting execution's scope variables, the run's durable env and the run's
//! own directory. The scope is never admitted, never persisted, and never
//! touches the aggregate: the caller turns what it read into plan values.

use crate::runtime::Runtime;
use crate::runtime::scheduler::{Context, Process, Task};
use crate::{Result, Vars, model::Workflow};
use std::sync::Arc;

/// The variable space one headless scope sees.
pub(crate) struct ScopeSpec {
    pub(crate) process_id: String,
    /// The acting node id (the kernel execution's task id).
    pub(crate) task_id: String,
    /// The declaring model, for the headless scope's tree.
    pub(crate) model: Workflow,
    /// The acting execution's scope chain (nearest wins) as JSON strings,
    /// seeded into the headless task so `$`-expressions resolve.
    pub(crate) vars: Vec<(String, String)>,
    /// The run's durable env map (`__env`), seeded into the headless process
    /// env so `$env.X` answers what an earlier effect wrote through
    /// `Context::set_env` — the legacy process env, shared by every task.
    pub(crate) env: Vec<(String, serde_json::Value)>,
    /// The run's caller authority: the legacy process's `PROC_OWNER`. The
    /// headless scope seals snapshot data under it (a run nobody sealed
    /// carries `deny_all`, so it reads nothing).
    pub(crate) owner: crate::ScopePolicy,
}

/// Build the headless scope: a bare in-memory [`Process`] over the declaring
/// model with one task on the acting node, the run's authority, directory and
/// env seeded, and the acting scope's variables in the task's data.
pub(crate) fn headless_scope(
    runtime: &Arc<Runtime>,
    spec: &ScopeSpec,
) -> Result<(Arc<Process>, Arc<Task>, Context)> {
    let process = Process::new(&spec.process_id, runtime);
    process.load(&spec.model)?;
    process.set_owner_scope(&spec.owner);
    if let Some(root) = runtime.config().workdir() {
        let dir = crate::runtime::prepare_workdir(&root, &spec.process_id)?;
        process.set_workdir(&dir);
    }
    if !spec.env.is_empty() {
        process.with_env_mut(|env| {
            for (name, value) in &spec.env {
                env.set(name, value.clone());
            }
        });
    }
    let node = process
        .tree()
        .node(&spec.task_id)
        .ok_or_else(|| crate::ActError::Runtime(format!("model has no node '{}'", spec.task_id)))?
        .clone();
    let task = process.create_task(&node, None)?;
    if !spec.vars.is_empty() {
        let mut vars = Vars::new();
        for (name, raw) in &spec.vars {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                vars.set(name, value.clone());
            }
        }
        task.update_data(&vars);
    }
    let ctx = process.create_context(&task);
    Ok((process, task, ctx))
}
