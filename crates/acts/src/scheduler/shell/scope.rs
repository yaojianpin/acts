//! The headless scope both evaluation seams run against.
//!
//! A package effect and a branch predicate are evaluated the way the legacy
//! engine evaluated them: on a *real* task of the declaring model, with the
//! acting execution's scope variables, the run's durable env and the run's
//! own directory. The scope is never admitted, never persisted, and never
//! touches the aggregate: the caller turns what it read into plan values.
//!
//! The carrier is deliberately not a [`Process`]/[`Task`] pair: those own a
//! whole [`NodeTree`](crate::scheduler::NodeTree) and a task graph, while the
//! two seams read one node's content and one map of variables. [`ScopeTask`]
//! holds exactly that (plus the cancellation token) and [`Scope`] the run
//! identity, env and directory, so an effect/predicate hop builds no tree.

use crate::{
    NodeKind, Result, ScopePolicy, Vars, Workflow,
    scheduler::{
        LegacyTaskState, NodeContent, Runtime,
        shell::{Context, Process, Task},
    },
    utils::{self, consts},
};

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// The variable space one headless scope sees.
pub(crate) struct ScopeSpec {
    pub(crate) process_id: String,
    /// The acting node id (the model node the execution runs).
    pub(crate) node: String,
    /// The acting execution's identity: `task_id.wire(process_id)` is the
    /// external id (`tid`) a package's forwarded action addresses since C5 —
    /// the node id no longer names an instance.
    pub(crate) task_id: crate::scheduler::core::model::task_graph::TaskId,
    /// The declaring model, for the headless scope's node.
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
    pub(crate) owner: ScopePolicy,
}

/// The lightweight process-side view a headless scope needs.
///
/// It answers the legacy `Process` surface the evaluation seams read — the
/// run id, its env (`$env` + the engine-private keys), the owner authority
/// and the workdir — without a [`NodeTree`](crate::scheduler::NodeTree). A
/// context built from a loaded read-model process (`Process::create_context`)
/// keeps delegating to that process; the headless path never has one.
pub struct Scope {
    id: String,
    env: crate::ShareLock<Vars>,
    source: Option<Arc<Process>>,
}

impl Scope {
    /// A headless scope: the run id plus the env the seam seeded (owner,
    /// workdir, durable env map).
    pub(crate) fn headless(id: &str, env: Vars) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            env: Arc::new(RwLock::new(env)),
            source: None,
        })
    }

    /// The read-model compatibility view: a context built from a loaded
    /// `Process` sees the process's own env and task rows.
    pub(crate) fn from_process(process: &Arc<Process>) -> Arc<Self> {
        Arc::new(Self {
            id: process.id().to_string(),
            env: Arc::new(RwLock::new(Vars::new())),
            source: Some(process.clone()),
        })
    }

    pub fn id(&self) -> &str {
        match &self.source {
            Some(process) => process.id(),
            None => &self.id,
        }
    }

    pub fn env(&self) -> Vars {
        match &self.source {
            Some(process) => process.env(),
            None => self.env.read().clone(),
        }
    }

    pub fn with_env<T, F: FnOnce(&Vars) -> T>(&self, f: F) -> T
    where
        T: for<'de> serde::Deserialize<'de> + Clone,
    {
        match &self.source {
            Some(process) => process.with_env(f),
            None => f(&self.env.read()),
        }
    }

    pub fn with_env_mut<F: FnOnce(&mut Vars)>(&self, f: F) {
        match &self.source {
            Some(process) => process.with_env_mut(f),
            None => f(&mut self.env.write()),
        }
    }

    /// The owner's snapshot scope authority (the legacy `PROC_OWNER`).
    pub(crate) fn owner_scope(&self) -> ScopePolicy {
        match &self.source {
            Some(process) => process.owner_scope(),
            None => self
                .with_env(|env| env.get::<ScopePolicy>(consts::PROC_OWNER))
                .unwrap_or_default(),
        }
    }

    /// The process's own directory (`<workdir>/<pid>`), or `None` when the
    /// engine config declares no workdir root.
    pub fn workdir(&self) -> Option<std::path::PathBuf> {
        self.with_env(|env| env.get::<std::path::PathBuf>(consts::PROC_WORKDIR))
    }

    /// The legacy `Process::set_data` write (`$set_process_var`). The
    /// headless scope has no root task, so the write lands nowhere — the
    /// same thing the legacy headless process did; a read-model context
    /// delegates to the process it was built from.
    pub fn set_data(&self, vars: &Vars) {
        if let Some(process) = &self.source {
            process.set_data(vars);
        }
    }

    /// Every read-model task whose predicate holds (declaration order, then
    /// start time — the legacy `Process::find_tasks` order).
    pub fn find_tasks(&self, predicate: impl Fn(&Arc<ScopeTask>) -> bool) -> Vec<Arc<ScopeTask>> {
        let Some(process) = &self.source else {
            return Vec::new();
        };
        process
            .find_tasks(|task| predicate(&ScopeTask::from_task(task.clone())))
            .into_iter()
            .map(ScopeTask::from_task)
            .collect()
    }

    pub fn tasks(&self) -> Vec<Arc<ScopeTask>> {
        let Some(process) = &self.source else {
            return Vec::new();
        };
        process
            .tasks()
            .into_iter()
            .map(ScopeTask::from_task)
            .collect()
    }

    pub fn task(&self, tid: &str) -> Option<Arc<ScopeTask>> {
        self.source.as_ref()?.task(tid).map(ScopeTask::from_task)
    }
}

/// The node content a headless task needs: exactly what the legacy `Task`
/// read off its `Arc<Node>` (id, content, kind), without the tree.
pub struct ScopeNode {
    id: String,
    content: NodeContent,
}

impl ScopeNode {
    fn new(id: &str, content: NodeContent) -> Self {
        Self {
            id: id.to_string(),
            content,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn kind(&self) -> NodeKind {
        match &self.content {
            NodeContent::Workflow(_) => NodeKind::Workflow,
            NodeContent::Branch(_) => NodeKind::Branch,
            NodeContent::Step(_) => NodeKind::Step,
            NodeContent::Act(_) => NodeKind::Act,
        }
    }
}

/// The lightweight task-side view a headless scope needs.
///
/// It is equivalent to the `Task` surface the seams read: `pid`/`id`,
/// `options`/`node`/`params`, `data` and `sealed`, the cancellation token,
/// and the small read helpers (`inputs`, `outputs`, `find`, `parent`). No
/// task graph, no runtime handle, no snapshot/restore surface.
pub struct ScopeTask {
    pub pid: String,
    pub id: String,
    node: Arc<ScopeNode>,
    data: crate::ShareLock<Vars>,
    sealed: crate::ShareLock<Vars>,
    state: LegacyTaskState,
    start_time: i64,
    end_time: i64,
    cancel: CancellationToken,
    /// The read-model compatibility view (`Process::create_context`): the
    /// wrapped task owns the data, sealed values and lineage; the headless
    /// path has none.
    source: Option<Arc<Task>>,
}

impl ScopeTask {
    /// The headless task: the acting execution's wire id on the acting node,
    /// with a fresh child of the runtime's shutdown token — the legacy
    /// `Task::new` contract.
    pub(crate) fn new(
        process_id: &str,
        id: &str,
        node: ScopeNode,
        runtime: &Arc<Runtime>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pid: process_id.to_string(),
            id: id.to_string(),
            node: Arc::new(node),
            data: Arc::new(RwLock::new(Vars::new())),
            sealed: Arc::new(RwLock::new(Vars::new())),
            state: LegacyTaskState::None,
            start_time: 0,
            end_time: 0,
            cancel: runtime.shutdown_token().child_token(),
            source: None,
        })
    }

    /// The read-model compatibility view: delegates every mutable read to the
    /// task a loaded `Process` owns, so an existing context keeps observing
    /// its process's live data.
    pub(crate) fn from_task(task: Arc<Task>) -> Arc<Self> {
        let node = Arc::new(ScopeNode::new(
            task.node().id(),
            task.node().content.clone(),
        ));
        Arc::new(Self {
            pid: task.pid.clone(),
            id: task.id.clone(),
            node,
            data: Arc::new(RwLock::new(Vars::new())),
            sealed: Arc::new(RwLock::new(Vars::new())),
            state: task.state(),
            start_time: task.start_time(),
            end_time: task.end_time(),
            cancel: task.cancellation_token(),
            source: Some(task),
        })
    }

    fn source<'a>(&'a self, scope: &'a Arc<Scope>) -> Option<(&'a Arc<Task>, &'a Arc<Process>)> {
        Some((self.source.as_ref()?, scope.source.as_ref()?))
    }

    pub fn node(&self) -> &Arc<ScopeNode> {
        &self.node
    }

    pub fn data(&self) -> Vars {
        match &self.source {
            Some(task) => task.data(),
            None => self.data.read().clone(),
        }
    }

    pub fn set_data_with<F: Fn(&mut Vars)>(&self, f: F) {
        match &self.source {
            Some(task) => task.set_data_with(f),
            None => f(&mut self.data.write()),
        }
    }

    pub fn set_data(&self, vars: &Vars) {
        match &self.source {
            Some(task) => task.set_data(vars),
            None => {
                let mut data = self.data.write();
                for (name, value) in vars.iter() {
                    data.set(name, value);
                }
            }
        }
    }

    pub fn update_data(&self, vars: &Vars) {
        self.set_data(vars);
    }

    pub(crate) fn set_sealed(&self, name: &str, value: Vars) {
        match &self.source {
            Some(task) => task.set_sealed(name, value),
            None => {
                self.sealed.write().set(name, value);
            }
        }
    }

    pub(crate) fn has_sealed_local(&self, name: &str) -> bool {
        match &self.source {
            Some(task) => task.has_sealed_local(name),
            None => self.sealed.read().get_value(name).is_some(),
        }
    }

    pub fn sealed(&self, scope: &Arc<Scope>, name: &str) -> Option<Vars> {
        match self.source(scope) {
            Some((task, process)) => task.sealed(process, name),
            None => self.sealed.read().get::<Vars>(name),
        }
    }

    pub fn find<T>(&self, scope: &Arc<Scope>, name: &str) -> Option<T>
    where
        T: DeserializeOwned + std::fmt::Debug + Clone,
    {
        match self.source(scope) {
            Some((task, process)) => task.find(process, name),
            None => self.data().get(name),
        }
    }

    pub fn parent(&self, scope: &Arc<Scope>) -> Option<Arc<ScopeTask>> {
        match self.source(scope) {
            Some((task, process)) => task.parent(process).map(ScopeTask::from_task),
            None => None,
        }
    }

    fn prev_id(&self) -> Option<String> {
        match &self.source {
            Some(task) => task.prev_id(),
            None => None,
        }
    }

    pub fn inputs(&self, ctx: &Context) -> Vars {
        let mut inputs = Vars::new();
        if let Some(prev) = self.prev_id()
            && let Some(prev_task) = ctx.proc.task(&prev)
        {
            // The previous scope's outputs are read in its own context: the
            // exposes/expressions it fills are its own.
            let prev_ctx =
                Context::from_scope(ctx.runtime.clone(), ctx.proc.clone(), prev_task.clone());
            for (k, v) in prev_task.outputs(&prev_ctx).iter() {
                inputs.set(k, v.clone());
            }
        }
        let vars = utils::fill_inputs(&self.node.content.vars(), ctx);
        inputs.extend(vars)
    }

    pub fn outputs(&self, ctx: &Context) -> Vars {
        let mut outputs = Vars::new();
        let mut exposes = self.node.content.exposes().clone();
        if exposes.is_empty() {
            // fallback: check options for exposes (runtime push actions)
            if let Some(opt_exposes) = self.options().get::<Vec<crate::Variant>>("exposes") {
                exposes = opt_exposes;
            }
        }
        if !exposes.is_empty() {
            for var in &exposes {
                outputs.set(&var.name, var.value.clone());
            }
        } else {
            // export all data except the private ones
            for (key, _) in self.data().iter() {
                if !consts::is_private_key(key) {
                    outputs.set(key, serde_json::json!(null))
                }
            }
        }

        utils::fill_outputs(&outputs, ctx)
    }

    pub fn options(&self) -> Vars {
        self.node.content.options()
    }

    pub fn params(&self, ctx: &Context) -> serde_json::Value {
        utils::fill_params(&self.node.content.params(), ctx)
    }

    pub fn state(&self) -> LegacyTaskState {
        match &self.source {
            Some(task) => task.state(),
            None => self.state.clone(),
        }
    }

    pub fn cost(&self) -> i64 {
        match &self.source {
            Some(task) => task.cost(),
            None => {
                if self.state.is_completed() {
                    return self.end_time - self.start_time;
                }
                utils::time::time_millis() - self.start_time
            }
        }
    }

    pub fn is_kind(&self, kind: NodeKind) -> bool {
        self.node.kind() == kind
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Build the headless scope: the run's env (owner, directory, durable env
/// map), the acting node's content and the acting execution's variables — no
/// `Process`, no `NodeTree`, no task graph.
pub(crate) fn headless_scope(
    runtime: &Arc<Runtime>,
    spec: &ScopeSpec,
) -> Result<(Arc<Scope>, Arc<ScopeTask>, Context)> {
    // The legacy order: the caller authority first, then the process
    // directory, then the durable env map (the same writes `Process::new` +
    // `set_owner_scope`/`set_workdir`/`with_env_mut` made).
    let mut env = Vars::new();
    env.set(consts::PROC_OWNER, spec.owner.clone());
    if let Some(root) = runtime.config().workdir() {
        let dir = crate::scheduler::prepare_workdir(&root, &spec.process_id)?;
        env.set(consts::PROC_WORKDIR, &dir);
    }
    for (name, value) in &spec.env {
        env.set(name, value.clone());
    }

    let content = node_content(&spec.model, &spec.node)
        .ok_or_else(|| crate::ActError::Runtime(format!("model has no node '{}'", spec.node)))?;
    let scope = Scope::headless(&spec.process_id, env);
    // The headless task is never durable or projected; its id is the acting
    // instance's wire, so a package that forwards a client command
    // (`Runtime::do_action`) addresses exactly the execution the scope was
    // built for.
    let node = ScopeNode::new(&spec.node, content);
    let task = ScopeTask::new(
        &spec.process_id,
        &spec.task_id.wire(&spec.process_id),
        node,
        runtime,
    );
    if !spec.vars.is_empty() {
        let mut vars = Vars::new();
        for (name, raw) in &spec.vars {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                vars.set(name, value.clone());
            }
        }
        task.update_data(&vars);
    }
    let ctx = Context::headless(runtime.clone(), scope.clone(), task.clone());
    Ok((scope, task, ctx))
}

/// The node content the legacy `NodeTree` would hold for `target`, resolved
/// straight from the declaring model.
///
/// It is the same map the tree build makes: the workflow id, every step
/// (top-level, branch, catch and timeout — recursively), and every branch
/// the build would enter (only when its step declares no `next`, the legacy
/// rule). The durable model is the imported one, whose ids and constraints
/// the import already assigned/checked.
fn node_content(model: &Workflow, target: &str) -> Option<NodeContent> {
    if model.id == target {
        return Some(NodeContent::Workflow(Arc::new(model.clone())));
    }
    model
        .steps
        .iter()
        .find_map(|step| node_content_in_step(step, target))
}

fn node_content_in_step(step: &crate::model::Step, target: &str) -> Option<NodeContent> {
    if step.id == target {
        return Some(NodeContent::Step(step.clone()));
    }
    for branch in crate::scheduler::shell::model::step_branches(step).unwrap_or_default() {
        if branch.id == target {
            return Some(NodeContent::Branch(branch.clone()));
        }
        if let Some(content) = branch
            .steps
            .iter()
            .find_map(|step| node_content_in_step(step, target))
        {
            return Some(content);
        }
    }
    for catch in &step.catches {
        if let Some(content) = node_content_in_step(catch, target) {
            return Some(content);
        }
    }
    for timeout in &step.timeouts {
        if let Some(content) = node_content_in_step(timeout, target) {
            return Some(content);
        }
    }
    None
}
