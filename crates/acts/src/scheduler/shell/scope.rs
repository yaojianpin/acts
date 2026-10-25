//! The headless scope both evaluation seams run against.
//!
//! A package effect and a branch predicate are evaluated the way the legacy
//! engine evaluated them: on a *real* task of the declaring model, with the
//! acting execution's scope variables, the run's durable env and the run's
//! own directory. The scope is never admitted, never persisted, and never
//! touches the aggregate: the caller turns what it read into plan values.
//!
//! The carrier is deliberately not a process/task pair: the two seams read one
//! node's content and one map of variables. [`ScopeTask`]
//! holds exactly that (plus the cancellation token) and [`Scope`] the run
//! identity, env and directory, so an effect/predicate hop builds no tree.
//!
//! The sealed map [`ScopeTask::set_sealed`] fills is per evaluation: it is
//! resolved at an effect's prepare and dropped with this scope. It is never
//! written to the aggregate or projected into a row; see the `snapshot`
//! module docs for the durability boundary.

use crate::{
    NodeKind, Result, ScopePolicy, Vars, Workflow,
    scheduler::{NodeContent, Runtime, core::model::task_graph::TaskState, shell::Context},
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
    /// The pushed act's synthetic step: the declaring model is
    /// immutable for the life of a run, so a pushed act's node is not in it —
    /// the act rides its execution (`__act`), and this is the step the
    /// headless scope resolves its params and options through. `None` for
    /// every declared node.
    pub(crate) pushed: Option<crate::model::Step>,
    /// Explicit local seed/overlay variables as JSON strings. Production
    /// effects leave this empty and resolve parent/root values from
    /// [`Self::snapshot`]; tests, read-model contexts and branch-predicate
    /// overlays use it as a local override.
    pub(crate) vars: Vec<(String, String)>,
    /// The shared aggregate the scope resolves parent/root values from on
    /// demand. `None` keeps the legacy eager `vars` seed for contexts that
    /// carry no snapshot (read model, unit tests, predicate fallback).
    pub(crate) snapshot: Option<Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
    /// The run's declared env map (`__env`), seeded into the headless process
    /// env so `$env.X` answers the workflow's `env` entries — read-only state,
    /// shared by every task.
    pub(crate) env: Vec<(String, serde_json::Value)>,
    /// The run's caller authority: the legacy process's `PROC_OWNER`. The
    /// headless scope seals snapshot data under it (a run nobody sealed
    /// carries `deny_all`, so it reads nothing).
    pub(crate) owner: ScopePolicy,
}

/// The lightweight process-side view a headless scope needs.
///
/// It answers the evaluation seams' process surface — the run id, its env
/// (`$env` + the engine-private keys), the owner authority and the workdir —
/// without a node tree or a task graph.
pub struct Scope {
    id: String,
    env: crate::ShareLock<Vars>,
    /// The shared aggregate a variable lookup walks (parent → root). `None`
    /// for a scope seeded with explicit `vars` only.
    snapshot: Option<Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
}

impl Scope {
    /// The headless scope: the run id plus the env the seam seeded (owner,
    /// workdir, durable env map) and, for an effect scope, the shared
    /// aggregate its expressions read lazily.
    pub(crate) fn headless(
        id: &str,
        env: Vars,
        snapshot: Option<Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            env: Arc::new(RwLock::new(env)),
            snapshot,
        })
    }

    /// Resolve one name from the acting execution's shared scope chain,
    /// nearest scope first. Values are decoded only when a name is read.
    fn find_in_chain<T>(
        &self,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        name: &str,
    ) -> Option<T>
    where
        T: DeserializeOwned,
    {
        let snapshot = self.snapshot.as_ref()?;
        let mut cursor = Some(task_id.clone());
        while let Some(id) = cursor {
            if let Some(raw) = snapshot.variables.get(&(id.clone(), name.to_string()))
                && scope_name_visible(name)
                && let Ok(value) = serde_json::from_str::<T>(raw)
            {
                return Some(value);
            }
            if let Some(private) = fanout_alias(name)
                && let Some(raw) = snapshot.variables.get(&(id.clone(), private.to_string()))
                && let Ok(value) = serde_json::from_str::<T>(raw)
            {
                return Some(value);
            }
            cursor = snapshot.tasks.get(&id).and_then(|task| task.parent.clone());
        }
        None
    }

    /// One step's own durable data, resolved on demand: the newest generation
    /// of `nid`, engine facts filtered, no full index built.
    fn step_data_of(&self, nid: &str) -> Option<Vars> {
        let snapshot = self.snapshot.as_ref()?;
        let task = newest_task(snapshot, nid)?;
        let mut data = Vars::new();
        for ((scope, name), raw) in &snapshot.variables {
            if scope == &task
                && !super::facts::is_engine_fact(name)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(raw)
            {
                data.set(name, value);
            }
        }
        Some(data)
    }

    /// One step's inputs, resolved on demand: predecessor outputs first, the
    /// step's own durable variables last (the legacy `Task::inputs`).
    fn step_inputs_of(&self, nid: &str) -> Option<Vars> {
        let snapshot = self.snapshot.as_ref()?;
        let task = newest_task(snapshot, nid)?;
        Some(Vars::from(super::facts::input_vars(snapshot, &task)))
    }

    /// The instantiated step ids, in deterministic order. Engine facts are
    /// not models, so the workflow/branch containers stay out.
    fn step_ids(&self) -> Vec<String> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        let Ok(Some(model)) = super::facts::model_of(snapshot) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = snapshot
            .tasks
            .keys()
            .filter(|id| super::planning::find_step_in_model(&model, id.node.as_str()).is_some())
            .map(|id| id.node.as_str().to_owned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn env(&self) -> Vars {
        self.env.read().clone()
    }

    pub fn with_env<T, F: FnOnce(&Vars) -> T>(&self, f: F) -> T
    where
        T: for<'de> serde::Deserialize<'de> + Clone,
    {
        f(&self.env.read())
    }

    /// Engine-only writes: the scope's env is built when the scope is created
    /// (owner, workdir, the run's declared env) and one private key is added
    /// for the effect seam. No package-facing setter exists.
    pub(crate) fn with_env_mut<F: FnOnce(&mut Vars)>(&self, f: F) {
        f(&mut self.env.write());
    }

    /// The owner's snapshot scope authority (the legacy `PROC_OWNER`).
    pub(crate) fn owner_scope(&self) -> ScopePolicy {
        self.with_env(|env| env.get::<ScopePolicy>(consts::PROC_OWNER))
            .unwrap_or_default()
    }

    /// The process's own directory (`<workdir>/<pid>`), or `None` when the
    /// engine config declares no workdir root.
    pub fn workdir(&self) -> Option<std::path::PathBuf> {
        self.with_env(|env| env.get::<std::path::PathBuf>(consts::PROC_WORKDIR))
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
        }
    }
}

/// The lightweight task-side view a headless scope needs.
///
/// It is equivalent to the task surface the seams read: `pid`/`id`,
/// `options`/`node`/`params`, `data` and `sealed`, the cancellation token,
/// and the small read helpers (`inputs`, `outputs`, `find`). No task graph,
/// no runtime handle, no snapshot/restore surface.
pub struct ScopeTask {
    pub pid: String,
    pub id: String,
    /// The execution's durable identity (node + generation), the anchor of
    /// the shared parent → root lookup.
    task_id: crate::scheduler::core::model::task_graph::TaskId,
    node: Arc<ScopeNode>,
    data: crate::ShareLock<Vars>,
    /// Snapshot data sealed for this evaluation only; never persisted.
    sealed: crate::ShareLock<Vars>,
    state: Option<TaskState>,
    start_time: i64,
    end_time: i64,
    cancel: CancellationToken,
}

impl ScopeTask {
    /// The headless task: the acting execution's wire id on the acting node,
    /// with a fresh child of the runtime's shutdown token — the legacy
    /// `Task::new` contract.
    pub(crate) fn new(
        process_id: &str,
        id: &str,
        task_id: crate::scheduler::core::model::task_graph::TaskId,
        node: ScopeNode,
        runtime: &Arc<Runtime>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pid: process_id.to_string(),
            id: id.to_string(),
            task_id,
            node: Arc::new(node),
            data: Arc::new(RwLock::new(Vars::new())),
            sealed: Arc::new(RwLock::new(Vars::new())),
            state: None,
            start_time: 0,
            end_time: 0,
            cancel: runtime.shutdown_token().child_token(),
        })
    }

    pub fn node(&self) -> &Arc<ScopeNode> {
        &self.node
    }

    pub fn data(&self) -> Vars {
        self.data.read().clone()
    }

    /// Engine-only: seed the acting execution's inputs.
    pub(crate) fn set_data_with<F: FnOnce(&mut Vars)>(&self, f: F) {
        f(&mut self.data.write());
    }

    /// Engine-only: seed the acting execution's scope chain. A package
    /// changes variables by returning its outputs, never by writing here.
    pub(crate) fn update_data(&self, vars: &Vars) {
        let mut data = self.data.write();
        for (name, value) in vars.iter() {
            data.set(name, value);
        }
    }

    pub(crate) fn set_sealed(&self, name: &str, value: Vars) {
        self.sealed.write().set(name, value);
    }

    pub(crate) fn has_sealed_local(&self, name: &str) -> bool {
        self.sealed.read().get_value(name).is_some()
    }

    pub fn sealed(&self, _scope: &Arc<Scope>, name: &str) -> Option<Vars> {
        self.sealed.read().get::<Vars>(name)
    }

    pub fn find<T>(&self, scope: &Arc<Scope>, name: &str) -> Option<T>
    where
        T: DeserializeOwned + std::fmt::Debug + Clone,
    {
        // The local overlay wins. A name present but not decodable as `T`
        // keeps the legacy `Task::find` behaviour and falls through to the
        // ancestor chain.
        if let Some(value) = self.data.read().get_value(name)
            && let Ok(value) = serde_json::from_value::<T>(value.clone())
        {
            return Some(value);
        }
        scope.find_in_chain(&self.task_id, name)
    }

    /// One step's own data, resolved from the shared aggregate on demand.
    pub fn step_data(&self, scope: &Arc<Scope>, nid: &str) -> Option<Vars> {
        scope.step_data_of(nid)
    }

    /// One step's inputs (predecessor outputs + own durable vars), resolved
    /// on demand.
    pub fn step_inputs(&self, scope: &Arc<Scope>, nid: &str) -> Option<Vars> {
        scope.step_inputs_of(nid)
    }

    /// The instantiated step ids, resolved on demand.
    pub fn steps(&self, scope: &Arc<Scope>) -> Vec<String> {
        scope.step_ids()
    }

    pub fn parent(&self, _scope: &Arc<Scope>) -> Option<Arc<ScopeTask>> {
        None
    }

    /// The acting execution's own declared vars, evaluated against the
    /// current scope. These are the values `prepare` seeds into the local
    /// map; inherited parent/root values stay in the shared chain.
    pub(crate) fn declared_inputs(&self, ctx: &Context) -> Vars {
        utils::fill_inputs(&self.node.content.vars(), ctx)
    }

    pub fn inputs(&self, ctx: &Context) -> Vars {
        // The legacy `Task::inputs` projection: the predecessor outputs first,
        // the step's own declared vars last (so they win on a name collision).
        // This is a read-only view for `$inputs()`; it is deliberately not
        // copied into the local map, where it would shadow a newer parent/root
        // value (a looping step's re-entry reads the previous iteration's
        // folded state from the chain).
        let mut inputs = ctx
            .proc
            .snapshot
            .as_ref()
            .map(|snapshot| Vars::from(super::facts::input_vars(snapshot, &self.task_id)))
            .unwrap_or_default();
        for (name, value) in &self.declared_inputs(ctx) {
            inputs.set(&name, value.clone());
        }
        inputs
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

    pub fn state(&self) -> Option<TaskState> {
        self.state
    }

    pub fn cost(&self) -> i64 {
        if self.state.is_some_and(TaskState::is_terminal) {
            return self.end_time - self.start_time;
        }
        utils::time::time_millis() - self.start_time
    }

    pub fn is_kind(&self, kind: NodeKind) -> bool {
        self.node.kind() == kind
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Whether a durable variable name is part of the expression-visible scope:
/// the private `__` engine facts stay hidden, except the legacy `__cost`
/// datum read by `$cost()`/`$cost_in(...)`.
fn scope_name_visible(name: &str) -> bool {
    !consts::is_private_key(name) || name == consts::TASK_COST
}

/// The public name a private fan-out fact answers as inside an item's scope.
fn fanout_alias(name: &str) -> Option<&'static str> {
    match name {
        "index" => Some(consts::FANOUT_INDEX_VAR),
        "value" => Some(consts::FANOUT_VALUE_VAR),
        _ => None,
    }
}

/// The newest generation of one model node in the shared aggregate.
fn newest_task(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    nid: &str,
) -> Option<crate::scheduler::core::model::task_graph::TaskId> {
    snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == nid)
        .max_by_key(|id| id.generation)
        .cloned()
}

/// Build the headless scope: the run's env (owner, directory, durable env
/// map), the acting node's content and the acting execution's variables — no
/// `Process`, no node tree, no task graph.
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

    let content = match node_content(&spec.model, &spec.node) {
        Some(content) => content,
        // A pushed act's node exists only as its own fact (see
        // [`ScopeSpec::pushed`]).
        None => NodeContent::Step(spec.pushed.clone().ok_or_else(|| {
            crate::ActError::Runtime(format!("model has no node '{}'", spec.node))
        })?),
    };
    let scope = Scope::headless(&spec.process_id, env, spec.snapshot.clone());
    // The headless task is never durable or projected; its id is the acting
    // instance's wire, so a package that forwards a client command
    // (`Runtime::do_action`) addresses exactly the execution the scope was
    // built for.
    let node = ScopeNode::new(&spec.node, content);
    let task = ScopeTask::new(
        &spec.process_id,
        &spec.task_id.wire(&spec.process_id),
        spec.task_id.clone(),
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

/// The node content the legacy tree build would have held for `target`,
/// resolved straight from the declaring model.
///
/// It is the same map the tree build made: the workflow id, every step
/// (top-level, branch, catch and timeout — recursively), and every branch
/// the build would enter (only when its step declares no `next`, the legacy
/// rule). The durable model is the imported one, whose ids and constraints
/// the import already assigned/checked.
pub(super) fn node_content(model: &Workflow, target: &str) -> Option<NodeContent> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::task_graph::TaskId;
    use crate::store::MemoryStore;
    use crate::{Config, NodeKind, Vars, model::Workflow};

    fn runtime() -> Arc<Runtime> {
        Runtime::new(&Config::default(), Some(Arc::new(MemoryStore::new()))).unwrap()
    }

    fn model() -> Workflow {
        Workflow::new()
            .with_id("wf-scope")
            .with_name("scope model")
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses("test.irq", Vars::new())
                    .with_var("own", 2)
                    .with_expose(crate::Variant {
                        name: "exposed".into(),
                        value: serde_json::json!("${{ v }}"),
                        ..crate::Variant::default()
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("true")
                            .with_step(|inner| inner.with_id("inner"))
                    })
                    .with_catch(|catch| catch.with_id("c1"))
                    .with_timeout(|timeout| timeout.with_id("t1"))
            })
            .with_step(|step| step.with_id("plain"))
            .with_step(|step| {
                step.with_id("opt").with_option(
                    "exposes",
                    vec![crate::Variant {
                        name: "from_options".into(),
                        value: serde_json::json!(5),
                        ..crate::Variant::default()
                    }],
                )
            })
            .with_step(|step| {
                step.with_id("params").with_uses(
                    "test.irq",
                    Vars::new().with("p", "${{ v }}").with("plain", "text"),
                )
            })
    }

    fn spec(node: &str) -> ScopeSpec {
        ScopeSpec {
            process_id: "p-scope".into(),
            node: node.into(),
            task_id: TaskId::new(node, 0).unwrap(),
            model: model(),
            pushed: None,
            vars: vec![
                ("v".into(), "1".into()),
                ("__private".into(), "2".into()),
                ("broken".into(), "{not json".into()),
            ],
            snapshot: None,
            env: vec![("declared".into(), serde_json::json!("from-env"))],
            owner: crate::ScopePolicy::deny_all(),
        }
    }

    /// The lightweight process view answers its surface without a tree.
    #[test]
    fn a_headless_process_reads_its_id_env_owner_and_workdir() {
        let scope = Scope::headless(
            "p",
            Vars::new().with("k", "v").with(
                crate::utils::consts::PROC_OWNER,
                crate::ScopePolicy::deny_all(),
            ),
            None,
        );
        assert_eq!(scope.id(), "p");
        assert_eq!(scope.env().get::<String>("k").as_deref(), Some("v"));
        assert_eq!(scope.with_env(|env| env.get::<String>("k")).unwrap(), "v");
        assert_eq!(scope.owner_scope(), crate::ScopePolicy::deny_all());
        assert!(scope.workdir().is_none());
        scope.with_env_mut(|env| env.set("added", true));
        assert_eq!(scope.with_env(|env| env.get::<bool>("added")), Some(true));
    }

    /// Every node shape resolves to its own kind and id.
    #[test]
    fn node_content_resolves_every_shape() {
        assert_eq!(
            node_content(&model(), "wf-scope").unwrap().kind(),
            NodeKind::Workflow
        );
        assert_eq!(node_content(&model(), "s1").unwrap().kind(), NodeKind::Step);
        assert_eq!(
            node_content(&model(), "b1").unwrap().kind(),
            NodeKind::Branch
        );
        assert_eq!(
            node_content(&model(), "inner").unwrap().kind(),
            NodeKind::Step
        );
        assert_eq!(node_content(&model(), "c1").unwrap().kind(), NodeKind::Step);
        assert_eq!(node_content(&model(), "t1").unwrap().kind(), NodeKind::Step);
        assert!(node_content(&model(), "missing").is_none());
    }

    /// The headless task answers inputs, outputs, params, data, sealed data
    /// and its engine handles exactly like the tree task did.
    #[tokio::test]
    async fn a_headless_task_answers_the_task_surface() {
        let runtime = runtime();
        let (scope, task, ctx) = headless_scope(&runtime, &spec("s1")).unwrap();
        assert_eq!(task.pid, "p-scope");
        assert_eq!(task.node().id(), "s1");
        // Seeded vars: valid JSON only, and the fan-out private names stay out
        // of the public scope.
        assert_eq!(task.data().get::<i32>("v"), Some(1));
        // Private engine names are seeded like any other variable (the scope
        // chain carries them); the output surface is what filters them.
        assert_eq!(task.data().get::<i32>("__private"), Some(2));
        assert!(task.data().get::<serde_json::Value>("broken").is_none());
        assert_eq!(
            scope.env().get::<String>("declared").as_deref(),
            Some("from-env")
        );

        task.set_data_with(|data| data.set("other", 3));
        task.update_data(&Vars::new().with("v", 9));
        assert_eq!(task.find::<i32>(&scope, "other"), Some(3));
        assert_eq!(task.data().get::<i32>("v"), Some(9));
        assert!(task.parent(&scope).is_none());

        let inputs = task.inputs(&ctx);
        assert_eq!(inputs.get::<i32>("own"), Some(2));
        // Exposes are the output surface: the expression resolves over the
        // task's data.
        let outputs = task.outputs(&ctx);
        assert_eq!(outputs.get::<i32>("exposed"), Some(9));
        assert_eq!(task.options().get::<i32>("missing"), None);
        assert_eq!(task.state(), None);
        assert!(task.cost() >= 0);
        assert!(task.is_kind(NodeKind::Step));
        assert!(!task.is_kind(NodeKind::Act));
        assert!(!task.cancellation_token().is_cancelled());
    }

    /// A step's params resolve through the same expression path, against the
    /// acting execution's data.
    #[tokio::test]
    async fn params_resolve_against_the_headless_scope() {
        let runtime = runtime();
        let (_scope, task, ctx) = headless_scope(&runtime, &spec("params")).unwrap();
        let params = task.params(&ctx);
        assert_eq!(params["p"].as_i64(), Some(1));
        assert_eq!(params["plain"].as_str(), Some("text"));
    }

    /// Without declarations the task exports its own non-private data, and an
    /// `exposes` option is the runtime push fallback.
    #[tokio::test]
    async fn outputs_fall_back_to_data_and_options() {
        let runtime = runtime();
        let (_scope, task, ctx) = headless_scope(&runtime, &spec("plain")).unwrap();
        task.set_data_with(|data| {
            data.set("v", 4);
            data.set("__private", 5);
        });
        let outputs = task.outputs(&ctx);
        assert_eq!(outputs.get::<i32>("v"), Some(4));
        assert!(outputs.get_value("__private").is_none());

        let (_scope, task, ctx) = headless_scope(&runtime, &spec("opt")).unwrap();
        let outputs = task.outputs(&ctx);
        assert_eq!(outputs.get::<i32>("from_options"), Some(5));
    }

    /// A sealed value is local to the headless task: it is readable and never
    /// leaks through `find`.
    #[tokio::test]
    async fn sealed_data_is_local_to_the_evaluation() {
        let runtime = runtime();
        let (scope, task, _ctx) = headless_scope(&runtime, &spec("s1")).unwrap();
        assert!(!task.has_sealed_local("profile"));
        task.set_sealed("profile", Vars::new().with("uid", "u1"));
        assert!(task.has_sealed_local("profile"));
        assert_eq!(
            task.sealed(&scope, "profile")
                .unwrap()
                .get::<String>("uid")
                .as_deref(),
            Some("u1")
        );
        assert!(task.find::<Vars>(&scope, "profile").is_none());
    }

    /// An aggregate model without the acting node is an engine error naming
    /// the node, not an empty scope.
    #[tokio::test]
    async fn an_unknown_node_is_refused() {
        let runtime = runtime();
        match headless_scope(&runtime, &spec("ghost")) {
            Ok(_) => panic!("an unknown node must not produce a scope"),
            Err(error) => {
                assert!(
                    format!("{error}").contains("model has no node 'ghost'"),
                    "{error}"
                )
            }
        }
    }
}
