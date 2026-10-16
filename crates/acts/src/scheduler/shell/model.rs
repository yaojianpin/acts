//! Declarative process specification for the kernel host.
//!
//! P1 seeded a flat workflow: one root and a declared child chain with
//! optional explicit join edges. The nested import (P2) keeps the same
//! vocabulary — `crate::scheduler::core::model::model_graph::ModelGraph`/`crate::scheduler::core::model::task_graph::TaskGraph` — for the
//! seeded top chain, and carries the nested structure (branch subtrees) as
//! durable aggregate variables (`__model`, `__auto`): the fan-out hop reads
//! them from the snapshot, so recovery replays never need the declaring
//! spec. Branch executions and their inner chains come alive through
//! `crate::scheduler::core::plan::container::ContainerPlanner` fan-outs, never at seed time.

use crate::{
    ActError, Result,
    scheduler::core::{
        commit::aggregate::{ProcessSnapshot, TaskSnapshot},
        model::{
            model_graph::{
                ChildRole, ModelGraph, ModelGraphBuilder, NodeDefinition, NodeId, NodeKind,
            },
            operation::{OperationId, OperationKind, OperationRecord},
            task_graph::{TaskGraph, TaskGraphBuilder, TaskId, TaskNode, TaskState},
        },
    },
};

fn runtime(error: impl std::fmt::Display) -> ActError {
    ActError::Runtime(error.to_string())
}

fn node_id(name: &str) -> Result<NodeId> {
    NodeId::parse(name).map_err(runtime)
}

/// The root-scope variable holding the declaring workflow (JSON). The
/// aggregate is the graph of record, so the nested model — which branches and
/// inner chains a fan-out must admit — replays from here without the spec.
pub(crate) const MODEL_VAR: &str = "__model";

/// The root-scope variable mapping every step node id to its durable
/// auto/waiting decision (`is_auto` at import time). Dynamic fan-outs seed a
/// created execution's `auto` variable from it.
pub(crate) const AUTO_MAP_VAR: &str = "__auto";

/// A flat process: `root` with `children` in declaration order (implicit
/// successor chain) plus optional explicit edges, e.g. a join that both
/// branches point at. Every node becomes one execution at generation 0.
///
/// `auto` names the tasks whose execution self-completes (no waiting
/// package); every other task stays `Running` until
/// [`KernelHost::apply_action`] applies a client action. The set is seeded
/// into the aggregate as the per-execution `auto` variable, so recovery
/// replays never need the declaring spec.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProcessSpec {
    pub(crate) process_id: String,
    pub(crate) root: String,
    pub(crate) children: Vec<String>,
    pub(crate) explicit_next: Vec<(String, String)>,
    pub(crate) auto: std::collections::BTreeSet<String>,
    /// Extra seed variables: `(scope, name, json-value)` where `None` scope
    /// is the root generation 0 and `Some(node)` a seeded step's generation 0.
    /// Root carries the workflow vars, the model itself and the auto map; a
    /// step carries its declared vars.
    pub(crate) seed_variables: Vec<(Option<String>, String, String)>,
    /// The caller authority the run carries — the legacy process env's
    /// `PROC_OWNER` value. The effect seam's headless scope re-seals snapshot
    /// data under it, so a run reads only the scopes its starter owns.
    /// [`crate::ScopePolicy::default`] (deny_all) is what a run nobody
    /// authenticated carries, exactly like the legacy process without the env
    /// key.
    pub(crate) owner: crate::ScopePolicy,
}

/// Assign the ids the legacy tree assigns before it builds nodes
/// (`prepare_workflow_ids` → `prepare_step_ids`/`prepare_branch_ids`): an
/// unnamed step, branch, catch handler or timeout branch gets a fresh short id,
/// and an unnamed workflow a long one (the `mid` every message carries). The
/// kernel derives every node id from the model, so this runs before the import;
/// the result is what lands in the aggregate as the durable model, which is how
/// a hop addresses a node the model never named.
fn assign_ids(workflow: &mut crate::model::Workflow) {
    if workflow.id.is_empty() {
        workflow.id = crate::utils::longid();
    }
    for step in workflow.steps.iter_mut() {
        assign_step_ids(step);
    }
}

fn assign_step_ids(step: &mut crate::model::Step) {
    if step.id.is_empty() {
        step.id = crate::utils::shortid();
    }
    for branch in step.branches.iter_mut() {
        assign_branch_ids(branch);
    }
    for catch in step.catches.iter_mut() {
        assign_step_ids(catch);
    }
    for timeout in step.timeouts.iter_mut() {
        assign_step_ids(timeout);
    }
}

fn assign_branch_ids(branch: &mut crate::model::Branch) {
    if branch.id.is_empty() {
        branch.id = crate::utils::shortid();
    }
    for step in branch.steps.iter_mut() {
        assign_step_ids(step);
    }
}

/// The `uses` ids of the container packages whose inline `acts` the kernel
/// materializes into model children (see [`materialize_container_acts`]).
pub(crate) const BLOCK_PACKAGE: &str = "acts.core.block";
pub(crate) const PARALLEL_PACKAGE: &str = "acts.core.parallel";
pub(crate) const SEQUENCE_PACKAGE: &str = "acts.core.sequence";

/// Materialize the inline acts of the container packages into the model's own
/// shape, so the kernel's fan-out machinery drives them.
///
/// The kernel fans a container out from the *model*: a step's `branches` and a
/// branch's `steps` are its children. A container package's acts live in its
/// `params.acts` array instead, and the package's `ctx.build_acts` writes them
/// into the headless scope the effect seam discards — so without this pass a
/// `acts.core.block`/`sequence`/`parallel` step resolves its package and
/// completes with no children at all.
///
/// The rewrite mirrors the legacy expansion (each package's `execute` built the
/// same shape through `ctx.build_acts`):
///
/// - `acts.core.block{ mode: sequence }` → one always-selected branch
///   `<step>#acts` whose steps are the acts, in declaration order (the branch
///   fans its steps out one successor at a time);
/// - `acts.core.block{ mode: parallel }` → one always-selected branch per act
///   (`<step>#act<i>`), which the container admits together;
/// - `acts.core.parallel{ in, acts }` / `acts.core.sequence{ in, acts }` → one
///   always-selected branch `<step>#in` holding a single template step
///   `<step>#item` (an `acts.core.block` over the same acts). The container
///   fans the template out once per `in` item at run time — only the run knows
///   how many there are — and the template's acts are materialized in turn by
///   the descent below.
///
/// The container step keeps its own package and params: it still announces
/// itself (the legacy step message) and its params still resolve, so a
/// schema violation fails the act exactly where it always did.
fn materialize_container_acts(workflow: &mut crate::model::Workflow) {
    for step in workflow.steps.iter_mut() {
        materialize_step(step);
    }
}

fn materialize_step(step: &mut crate::model::Step) {
    materialize_one(step);
    // The branches just added are walked too: a container act nested inside
    // another container's acts is materialized in turn, and its own branches
    // after that.
    for branch in step.branches.iter_mut() {
        for inner in branch.steps.iter_mut() {
            materialize_step(inner);
        }
    }
    for catch in step.catches.iter_mut() {
        materialize_step(catch);
    }
    for timeout in step.timeouts.iter_mut() {
        materialize_step(timeout);
    }
}

/// One step's inline acts, materialized into its branches when it is a
/// container package step and it declares an `acts` array. Anything else
/// (another package, no acts) is left alone.
fn materialize_one(step: &mut crate::model::Step) {
    let Some(uses) = step.uses.clone() else {
        return;
    };
    let params = step.params.clone();
    let Some(object) = params.as_object() else {
        return;
    };
    let Some(acts) = object.get("acts") else {
        return;
    };
    let Some(values) = acts.as_array() else {
        return;
    };
    let Some(steps) = values.iter().map(act_to_step).collect::<Option<Vec<_>>>() else {
        // An act the model cannot read is not silently dropped: the step keeps
        // its declaration and the package's own params validation decides it.
        return;
    };
    let id = step.id.clone();
    let branches = match uses.as_str() {
        BLOCK_PACKAGE => {
            let parallel = object.get("mode").and_then(|mode| mode.as_str()) == Some("parallel");
            if parallel {
                steps
                    .into_iter()
                    .enumerate()
                    .map(|(index, step)| container_branch(format!("{id}#act{index}"), vec![step]))
                    .collect()
            } else {
                vec![container_branch(format!("{id}#acts"), steps)]
            }
        }
        PARALLEL_PACKAGE | SEQUENCE_PACKAGE => {
            let mode = if uses == PARALLEL_PACKAGE {
                "parallel"
            } else {
                "sequence"
            };
            let template = crate::model::Step {
                id: format!("{id}#item"),
                uses: Some(BLOCK_PACKAGE.to_string()),
                params: serde_json::json!({ "mode": mode, "acts": acts }),
                ..Default::default()
            };
            vec![container_branch(format!("{id}#in"), vec![template])]
        }
        _ => return,
    };
    step.branches.extend(branches);
}

/// One inline act as a model step: the same vocabulary, with the id the legacy
/// tree assigned (a fresh short id for an unnamed act, so its node is
/// addressable) and an empty `uses` normalized to `None` — a step without a
/// package self-completes, while `Some("")` would read as an unregistered
/// package and wait for a client that never comes.
fn act_to_step(act: &serde_json::Value) -> Option<crate::model::Step> {
    let act: crate::model::Act = serde_json::from_value(act.clone()).ok()?;
    let id = if act.id.trim().is_empty() {
        crate::utils::shortid()
    } else {
        act.id
    };
    Some(crate::model::Step {
        name: act.name,
        desc: act.desc,
        id,
        vars: act.vars,
        r#if: act.r#if,
        uses: (!act.uses.trim().is_empty()).then_some(act.uses),
        params: act.params,
        options: act.options,
        metadata: act.metadata,
        exposes: act.exposes,
        ..Default::default()
    })
}

/// An always-selected branch over `steps`. The kernel's branch selection reads
/// a missing guard as unselected (`PredicateValue::False`), so a materialized
/// container branch states its selection plainly.
fn container_branch(id: String, steps: Vec<crate::model::Step>) -> crate::model::Branch {
    crate::model::Branch {
        id,
        r#if: Some("true".to_string()),
        steps,
        ..Default::default()
    }
}

/// Whether a step self-completes once its act ran. The answer is the
/// package's own `ActRunAs`: `Func` and `Msg` runs come to an end inside the
/// execution, an `Irq` waits for a client response. A step without a package
/// self-completes; an unregistered package defaults to the `ActRunAs`
/// default (`Irq`) and waits.
fn is_auto(uses: Option<&str>, run_as: &dyn Fn(&str) -> Option<crate::ActRunAs>) -> bool {
    match uses {
        None => true,
        Some(id) => match run_as(id) {
            Some(crate::ActRunAs::Func | crate::ActRunAs::Msg) => true,
            Some(crate::ActRunAs::Irq) | None => false,
        },
    }
}

/// The branches a step actually runs, mirroring the legacy tree build: a
/// step that declares an explicit `next` ignores its branches.
pub(crate) fn step_branches(step: &crate::model::Step) -> Option<&[crate::model::Branch]> {
    match (step.next.as_ref(), step.branches.as_slice()) {
        (None, []) => None,
        (None, branches) => Some(branches),
        (Some(_), _) => None,
    }
}

/// Where one step sits in the imported model — what the import derives from
/// its position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepPlace {
    /// A seeded chain step of the workflow root: its auto decision joins the
    /// seed set. Its own `if`/`while` is its guard, evaluated by the start
    /// hop's entry check.
    TopLevel,
    /// A `catches` handler of a step. Its `if` is the handler's own
    /// selection — the legacy `Step::init` guard, evaluated when the failed
    /// step dispatches its handlers — so it is imported, not refused.
    Catch,
    /// A `timeouts` branch of a step. Its `if` is the branch's window, the
    /// expression the legacy `Step::init` evaluated when the tick dispatched
    /// the branch (`$cost_in(...)`), so it is imported and evaluated by the
    /// dispatcher; a `while` would loop the branch, which neither the tick nor
    /// the kernel has a driver for.
    Timeout,
    /// A step inside a branch: its guard is its own start hop's, like a
    /// top-level step's, and a guard that fails falls through to the next step
    /// declared in the same branch (`GUARD_SKIP_VAR` keeps the join from
    /// reading that skip as a dead path).
    Nested,
}

/// One step of the nested import walk: pin the step's auto decision and
/// declared vars, reject what the shadow cannot mean faithfully, and descend
/// into its branches and catches.
fn import_step(
    step: &crate::model::Step,
    run_as: &dyn Fn(&str) -> Option<crate::ActRunAs>,
    place: StepPlace,
    auto: &mut std::collections::BTreeSet<String>,
    auto_map: &mut serde_json::Map<String, serde_json::Value>,
    seed_variables: &mut Vec<(Option<String>, String, String)>,
) -> Result<()> {
    if step.id.trim().is_empty() {
        // `from_workflow` assigns the ids the legacy tree assigns, so only a
        // *blank* (whitespace) id reaches here: it cannot address a node, and
        // the legacy tree would keep it as-is (its `prepare_step_ids` tests
        // `is_empty`), which is a model the kernel refuses to invent a meaning
        // for.
        return Err(ActError::Runtime(
            "a nested step has a blank id; the kernel derives node ids from the model, and a blank one cannot address the node"
                .to_string(),
        ));
    }
    let step_auto = is_auto(step.uses.as_deref(), run_as);
    auto_map.insert(step.id.clone(), serde_json::Value::Bool(step_auto));
    if place == StepPlace::TopLevel && step_auto {
        auto.insert(step.id.clone());
    }
    // A seeded chain step's declared vars land on its own scope as a durable
    // fact. A nested node's scope does not exist until the hop that admits it,
    // and that hop seeds them from the model (`child_variables`), so seeding
    // them here would only create a variable on an execution the aggregate
    // refuses to know (the seed validates every scope it writes).
    if place == StepPlace::TopLevel {
        for var in &step.vars {
            seed_variables.push((
                Some(step.id.clone()),
                var.name.clone(),
                var.value.to_string(),
            ));
        }
    }
    // `while` cannot be combined with `next`: a while loop always flows to the
    // next declared step when its condition fails, so an explicit `next` could
    // never be taken (the same rule the model view enforces).
    if step.next.is_some() && step.r#while.is_some() {
        return Err(ActError::Runtime(format!(
            "step '{}' cannot combine 'while' with 'next': a while loop always flows to the next declared step when its condition fails",
            step.id
        )));
    }
    // A handler's `if` selects it (legacy `Step::init`); its `while` would
    // loop the handler, which needs step-loop machinery the kernel has no
    // driver for. A timeout branch is the same shape: its `if` is its window
    // (evaluated by the tick's dispatcher), its `while` would loop it.
    if matches!(place, StepPlace::Catch | StepPlace::Timeout) && step.r#while.is_some() {
        let kind = if place == StepPlace::Catch {
            "catch step"
        } else {
            "timeout step"
        };
        return Err(ActError::Runtime(format!(
            "{kind} '{}' carries a 'while' guard; looping handlers are not imported yet",
            step.id
        )));
    }
    // The legacy tree builds `catches` as Catch children and `timeouts` as
    // Timeout children; `Step::on_error` dispatches the handlers of a failed
    // step while the tick dispatches the branches of a running one — two
    // independent dispatch shapes over the same node, and a step may declare
    // both: the catch join gates on every child the step has (a timeout branch
    // the tick fired included) and decides the caught error from its CATCH
    // children alone (the legacy `is_empty_catched` filters by the sign).
    if !step.catches.is_empty() {
        for catch in &step.catches {
            import_step(
                catch,
                run_as,
                StepPlace::Catch,
                auto,
                auto_map,
                seed_variables,
            )?;
        }
    }
    // A step's timeout branches are dispatched by the tick, not by a fan-out:
    // the timed-out execution keeps running (or waiting) while the tick fires
    // them, one per evaluation round, in declaration order (see
    // `crate::scheduler::timeout`). They are imported (their guards included) and, like
    // a catch handler, may be a container over their own children.
    for timeout in &step.timeouts {
        import_step(
            timeout,
            run_as,
            StepPlace::Timeout,
            auto,
            auto_map,
            seed_variables,
        )?;
    }
    for branch in step_branches(step).unwrap_or_default() {
        if branch.id.trim().is_empty() {
            return Err(ActError::Runtime(format!(
                "branch of step '{}' has a blank id; the kernel derives node ids from the model, and a blank one cannot address the node",
                step.id
            )));
        }
        if !branch.needs.is_empty() {
            return Err(ActError::Runtime(format!(
                "branch '{}' declares 'needs'; branch dependencies are not imported yet",
                branch.id
            )));
        }
        for inner in &branch.steps {
            import_step(
                inner,
                run_as,
                StepPlace::Nested,
                auto,
                auto_map,
                seed_variables,
            )?;
        }
    }
    Ok(())
}

impl ProcessSpec {
    #[cfg(test)]
    pub(crate) fn linear(process_id: impl Into<String>, root: &str, children: &[&str]) -> Self {
        Self {
            process_id: process_id.into(),
            root: root.to_string(),
            children: children.iter().map(|name| name.to_string()).collect(),
            explicit_next: Vec::new(),
            auto: children.iter().map(|name| name.to_string()).collect(),
            seed_variables: Vec::new(),
            owner: crate::ScopePolicy::default(),
        }
    }

    /// Carry the run's caller authority (the legacy process env's
    /// `PROC_OWNER`): the effect seam seals snapshot data under it, so a run
    /// started by a restricted caller reads only that caller's scopes.
    pub(crate) fn with_owner(mut self, owner: crate::ScopePolicy) -> Self {
        self.owner = owner;
        self
    }

    /// Carry the run's start inputs (what the legacy start hands
    /// `Process::load_with_vars`): they seed the root scope *after* the
    /// workflow's own vars, so an input of the same name wins — exactly the
    /// order legacy appends them in. Without this a snapshot scope key that
    /// arrives as a start input (`uid` from `proc:start`) would be absent from
    /// the shadow and its target would never seal.
    pub(crate) fn with_vars(mut self, vars: &crate::Vars) -> Self {
        for (name, value) in vars {
            self.seed_variables.push((None, name, value.to_string()));
        }
        self
    }

    /// Import a workflow. The legacy root task id (`$`) is the root and the
    /// workflow's top-level steps seed one chain; a step's branches (legacy
    /// rule: only when the step declares no `next`) and their inner steps do
    /// NOT seed executions — they come alive when the branched step's effect
    /// hop fans out, selected by their `if` predicates. The nested structure,
    /// the workflow vars and the per-node auto decision all become durable
    /// root-scope variables, so the aggregate replays without the spec.
    ///
    /// Not imported (rejected instead of silently flattened into wrong
    /// semantics): branch `needs` (a dependency deferral the shadow has no
    /// re-evaluation driver for yet), `if`/`while` guards on steps inside a
    /// branch (the kernel join treats `Skipped` as a dead path, while a
    /// legacy step guard falls through to the chain — one state cannot mean
    /// both) and a step declaring both `catches` and `timeouts` (two child
    /// sets the kernel's catch join cannot tell apart).
    pub(crate) fn from_workflow(
        process_id: impl Into<String>,
        workflow: &crate::model::Workflow,
        run_as: &dyn Fn(&str) -> Option<crate::ActRunAs>,
    ) -> Result<Self> {
        // The caller's model stays untouched: the ids the legacy tree assigns
        // (`prepare_workflow_ids` before it builds nodes) are assigned on a
        // copy, and that copy is what the aggregate carries as its durable
        // model — so an unnamed nested node is addressable by the hops that
        // must reach it, exactly as it is in the legacy run.
        let mut assigned = workflow.clone();
        assign_ids(&mut assigned);
        // The container packages' inline acts become model children here,
        // after the ids they need are assigned and before anything derives the
        // nested structure from the copy (the durable model and the auto map).
        materialize_container_acts(&mut assigned);
        let workflow = &assigned;
        let mut seed_variables: Vec<(Option<String>, String, String)> = Vec::new();
        for var in &workflow.vars {
            seed_variables.push((None, var.name.clone(), var.value.to_string()));
        }
        // The model itself is part of the aggregate: the fan-out hop reads
        // the nested structure from the snapshot.
        seed_variables.push((
            None,
            MODEL_VAR.to_string(),
            serde_json::to_string(workflow).map_err(runtime)?,
        ));

        let mut auto_map = serde_json::Map::new();
        let mut auto = std::collections::BTreeSet::new();
        for step in &workflow.steps {
            import_step(
                step,
                run_as,
                StepPlace::TopLevel,
                &mut auto,
                &mut auto_map,
                &mut seed_variables,
            )?;
        }
        seed_variables.push((
            None,
            AUTO_MAP_VAR.to_string(),
            serde_json::Value::Object(auto_map).to_string(),
        ));

        Ok(Self {
            process_id: process_id.into(),
            root: crate::utils::consts::TASK_ROOT_TID.to_string(),
            children: workflow.steps.iter().map(|step| step.id.clone()).collect(),
            explicit_next: Vec::new(),
            auto,
            seed_variables,
            owner: crate::ScopePolicy::default(),
        })
    }

    pub(crate) fn model_graph(&self) -> Result<ModelGraph> {
        let root = node_id(&self.root)?;
        let mut builder = ModelGraphBuilder::default();
        builder
            .add_node(NodeDefinition {
                id: root.clone(),
                kind: NodeKind::Workflow,
                name: self.root.clone(),
            })
            .map_err(runtime)?;
        for child in &self.children {
            let id = node_id(child)?;
            builder
                .add_node(NodeDefinition {
                    id: id.clone(),
                    // The children are the model's top-level *steps*
                    // (`from_workflow` seeds them from `workflow.steps`), which
                    // is also what `node_kind_and_name` answers for these ids.
                    kind: NodeKind::Step,
                    name: child.clone(),
                })
                .map_err(runtime)?;
            builder
                .add_child(root.clone(), id, ChildRole::Normal)
                .map_err(runtime)?;
        }
        for (from, to) in &self.explicit_next {
            builder
                .set_explicit_next(node_id(from)?, node_id(to)?)
                .map_err(runtime)?;
        }
        // The declaration chain links siblings only; the root needs its own
        // edge to the first child or propagation can never leave the root.
        // An explicit edge out of the root wins.
        if let Some(first) = self.children.first()
            && !self
                .explicit_next
                .iter()
                .any(|(from, _)| from == &self.root)
        {
            builder
                .set_explicit_next(node_id(&self.root)?, node_id(first)?)
                .map_err(runtime)?;
        }
        builder.set_root(node_id(&self.root)?).map_err(runtime)?;
        builder.build().map_err(runtime)
    }

    /// The complete initial aggregate: every execution exists (root `Ready`,
    /// the rest `Created`), the graph is wired, and the root's `ExecuteTask`
    /// operation is the single durable seed. Creation is one atomic
    /// `repository.create`, never a plan.
    pub(crate) fn seed_snapshot(&self) -> Result<ProcessSnapshot> {
        self.seed_snapshot_from(TaskState::Ready)
    }

    /// The seed of a run that starts *parked*: exactly [`Self::seed_snapshot`]
    /// except that the root stays `Created`, which is what the read model
    /// writes as the `None` process state — the shape the legacy parked row
    /// has. Its `ExecuteTask` operation is driven only once a slot frees (see
    /// `crate::scheduler::core::plan::concurrency`), so the aggregate is
    /// otherwise indistinguishable from an admitted start's.
    pub(crate) fn seed_parked_snapshot(&self) -> Result<ProcessSnapshot> {
        self.seed_snapshot_from(TaskState::Created)
    }

    /// The seed both entry points share; `root_state` is the root execution's
    /// state (`Ready` for an admitted start, `Created` for a parked one).
    fn seed_snapshot_from(&self, root_state: TaskState) -> Result<ProcessSnapshot> {
        self.model_graph()?;
        let mut snapshot = ProcessSnapshot::empty(&self.process_id).map_err(runtime)?;
        snapshot.version.graph = 1;

        let root = TaskId::new(&self.root, 0).map_err(runtime)?;
        let mut tasks = std::collections::HashMap::new();
        tasks.insert(
            root.clone(),
            TaskSnapshot {
                model_node: node_id(&self.root)?,
                parent: None,
                predecessors: Vec::new(),
                state: root_state,
                revision: 0,
            },
        );
        for child in &self.children {
            let id = TaskId::new(child, 0).map_err(runtime)?;
            tasks.insert(
                id,
                TaskSnapshot {
                    model_node: node_id(child)?,
                    parent: Some(root.clone()),
                    predecessors: Vec::new(),
                    state: TaskState::Created,
                    revision: 0,
                },
            );
        }
        snapshot.tasks = tasks;
        // The auto/waiting decision is durable state, not spec state: the
        // aggregate replays without the model.
        for id in snapshot.tasks.keys() {
            let auto = id.node == self.root || self.auto.contains(&id.node);
            snapshot.variables.insert(
                (id.clone(), "auto".to_string()),
                if auto { "1" } else { "0" }.into(),
            );
        }
        // The declared model facts: workflow vars, the model itself, the auto
        // map on the root scope; each seeded step's declared vars on its own
        // scope. Dynamic fan-outs read them from here.
        for (scope, name, value) in &self.seed_variables {
            let key = match scope {
                None => root.clone(),
                Some(node) => TaskId::new(node, 0).map_err(runtime)?,
            };
            snapshot
                .variables
                .insert((key, name.clone()), value.clone());
        }
        // The run's caller authority, on the root scope like the legacy
        // process env's `PROC_OWNER`: the effect seam's headless scope re-seals
        // snapshot data under it (and a run nobody sealed carries deny_all).
        snapshot.variables.insert(
            (root.clone(), crate::utils::consts::PROC_OWNER.to_string()),
            serde_json::to_string(&self.owner).map_err(runtime)?,
        );
        // The run's creation stamp, on the root scope: the order a freed slot
        // admits parked runs in (see `core::plan::concurrency::CREATED_VAR`).
        // Taken here, where the clock lives — the aggregate carries it as the
        // durable fact the driver may sort by, like the legacy row's
        // `timestamp`.
        snapshot.variables.insert(
            (
                root.clone(),
                crate::scheduler::core::plan::concurrency::CREATED_VAR.to_string(),
            ),
            crate::utils::time::time_millis().to_string(),
        );
        // At creation the MODEL is the edge authority: every node's successor
        // becomes an execution edge, and the predecessor lists are its mirror.
        // From then on the snapshot itself is the graph of record.
        let model = self.model_graph()?;
        let by_node = |node: &NodeId| {
            snapshot
                .tasks
                .iter()
                .find(|(_, candidate)| &candidate.model_node == node)
                .map(|(id, _)| id.clone())
        };
        let mut edges: Vec<(TaskId, TaskId)> = Vec::new();
        for (id, task) in &snapshot.tasks {
            if let Some(successor) = model.successor(&task.model_node) {
                let Some(target) = by_node(successor) else {
                    return Err(ActError::Runtime("successor without execution".into()));
                };
                edges.push((id.clone(), target));
            }
        }
        for (from, to) in edges {
            if let Some(target) = snapshot.tasks.get_mut(&to) {
                target.predecessors.push(from);
            }
        }

        let operation_id = OperationId {
            process_id: self.process_id.clone(),
            node: self.root.clone(),
            generation: 0,
            sequence: 0,
        };
        snapshot.operations.insert(
            operation_id.clone(),
            OperationRecord::pending(operation_id, OperationKind::ExecuteTask),
        );
        snapshot.validate().map_err(runtime)?;
        Ok(snapshot)
    }
}

/// Rebuild the execution graph purely from the snapshot: the parent map
/// and predecessor lists in `TaskSnapshot` are the graph of record,
/// seeded from the model at creation and extended by every
/// `TaskCreate`. The root is the single parentless execution; the
/// revision is the aggregate's graph revision, which is what
/// `PropagationPlanner` checks.
pub(crate) fn task_graph(snapshot: &ProcessSnapshot) -> Result<TaskGraph> {
    let mut builder =
        TaskGraphBuilder::new(&snapshot.process_id, snapshot.version.graph).map_err(runtime)?;
    let mut root: Option<TaskId> = None;
    for (id, task) in &snapshot.tasks {
        builder
            .add_node(TaskNode {
                id: id.clone(),
                model_node: task.model_node.clone(),
                state: task.state,
                revision: task.revision,
            })
            .map_err(runtime)?;
        match &task.parent {
            Some(parent) => builder
                .add_child(parent.clone(), id.clone())
                .map_err(runtime)?,
            None => root = Some(id.clone()),
        }
    }
    let root = root.ok_or_else(|| ActError::Runtime("no root execution".into()))?;
    builder.set_root(root).map_err(runtime)?;
    for (id, task) in &snapshot.tasks {
        for predecessor in &task.predecessors {
            builder
                .add_successor(predecessor.clone(), id.clone())
                .map_err(runtime)?;
        }
    }
    builder.build().map_err(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(workflow: &crate::model::Workflow) -> Result<ProcessSpec> {
        ProcessSpec::from_workflow("p1", workflow, &|_| None)
    }

    /// The `examples/catches/model.yml` shape: an unnamed catch handler with
    /// unnamed branches and unnamed inner steps. The legacy tree assigns ids
    /// before it builds nodes (`prepare_step_ids`), and the import does the same
    /// on a copy — the ids land in the durable model, which is the only way a
    /// hop can address a node the model never named. The caller's model stays
    /// untouched.
    #[test]
    fn unnamed_nested_nodes_get_ids_like_the_legacy_tree() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                .with_catch(|catch| {
                    catch.with_branch(|branch| {
                        branch.with_if("$ecode() == 'e1'").with_step(|step| {
                            step.with_uses(crate::utils::test::USES_MSG, crate::Vars::new())
                        })
                    })
                })
        });
        let imported = spec(&workflow).unwrap();
        let snapshot = imported.seed_snapshot().unwrap();
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let model: crate::model::Workflow = serde_json::from_str(
            snapshot
                .variables
                .get(&(root.clone(), MODEL_VAR.to_string()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(model.steps.len(), 1);
        let catch = &model.steps[0].catches[0];
        assert!(!catch.id.is_empty(), "the handler got an id");
        let branch = &catch.branches[0];
        assert!(!branch.id.is_empty(), "the branch got an id");
        let inner = &branch.steps[0];
        assert!(!inner.id.is_empty(), "the inner step got an id");
        assert!(
            !model.id.is_empty(),
            "an unnamed workflow gets the id every message carries as `mid`"
        );
        // The auto map knows the assigned ids, so the fan-out that admits them
        // can read their self-completion decision.
        let auto: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            snapshot
                .variables
                .get(&(root, AUTO_MAP_VAR.to_string()))
                .unwrap(),
        )
        .unwrap();
        assert!(auto.contains_key(&catch.id), "auto map: {auto:?}");
        assert!(auto.contains_key(&inner.id), "auto map: {auto:?}");
        // The caller's model is not mutated.
        assert!(workflow.steps[0].catches[0].id.is_empty());
        assert!(workflow.steps[0].catches[0].branches[0].id.is_empty());
        assert!(workflow.id.is_empty());
    }

    fn branched() -> crate::model::Workflow {
        crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            })
    }

    #[test]
    fn the_run_owner_is_a_durable_root_fact() {
        let workflow = crate::model::Workflow::new().with_step(|step| step.with_id("step1"));
        let owner = crate::ScopePolicy {
            subject: "u1".to_string(),
            scopes: std::collections::HashMap::from([(
                "profile".to_string(),
                vec!["$subject".to_string()],
            )]),
            ..crate::ScopePolicy::deny_all()
        };
        let spec = spec(&workflow).unwrap().with_owner(owner.clone());
        let snapshot = spec.seed_snapshot().unwrap();
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let raw = snapshot
            .variables
            .get(&(root, crate::utils::consts::PROC_OWNER.to_string()))
            .expect("the owner is seeded on the root scope");
        assert_eq!(
            serde_json::from_str::<crate::ScopePolicy>(raw).unwrap(),
            owner
        );
    }

    #[test]
    fn start_inputs_seed_the_root_scope_after_the_workflow_vars() {
        let workflow = crate::model::Workflow::new()
            .with_var("uid", "declared")
            .with_step(|step| step.with_id("step1"));
        let spec = spec(&workflow)
            .unwrap()
            .with_vars(&crate::Vars::new().with("uid", "input").with("extra", 7));
        let snapshot = spec.seed_snapshot().unwrap();
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let var = |name: &str| {
            snapshot
                .variables
                .get(&(root.clone(), name.to_string()))
                .cloned()
        };
        assert_eq!(var("uid").as_deref(), Some("\"input\""), "an input wins");
        assert_eq!(var("extra").as_deref(), Some("7"));
    }

    #[test]
    fn nested_models_import_with_durable_model_facts() {
        let workflow = branched();
        let spec = spec(&workflow).unwrap();
        // Branches and their steps do not seed executions; the top chain does.
        assert_eq!(spec.children, vec!["step1".to_string()]);
        let snapshot = spec.seed_snapshot().unwrap();
        assert_eq!(snapshot.tasks.len(), 2, "root + step1 only");
        // The root scope carries the workflow vars, the model and the auto
        // map — the fan-out's durable inputs.
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let variable = |name: &str| {
            snapshot
                .variables
                .get(&(root.clone(), name.to_string()))
                .unwrap()
                .clone()
        };
        assert_eq!(variable("v"), "1");
        let model: crate::model::Workflow = serde_json::from_str(&variable(MODEL_VAR)).unwrap();
        assert_eq!(model.steps[0].branches.len(), 2);
        let auto: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&variable(AUTO_MAP_VAR)).unwrap();
        assert_eq!(auto["step1"], serde_json::Value::Bool(true));
        assert_eq!(auto["s11"], serde_json::Value::Bool(true));
    }

    #[test]
    fn branch_needs_are_rejected_not_silently_flattened() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_need("b0")
                    .with_step(|step| step.with_id("s11"))
            })
        });
        let error = spec(&workflow).unwrap_err();
        assert!(format!("{error}").contains("'b1' declares 'needs'"));
    }

    #[test]
    fn catch_and_timeout_declarations_import_and_loops_or_blank_ids_are_refused() {
        // A catch step is a nested step: its auto decision and declared vars
        // become durable facts like a branch's, but it seeds no execution —
        // it comes alive when the failed step fans it out, and that hop seeds
        // the vars from the model.
        let with_catch = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_var("own", 1)
                .with_uses("acts.transform.set", crate::Vars::new())
                .with_catch(|step| {
                    step.with_id("c1").with_var("v", 1).with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("$ecode() == 'err1'")
                            .with_step(|step| step.with_id("s11"))
                    })
                })
        });
        let imported = spec(&with_catch).unwrap();
        assert_eq!(imported.children, vec!["s1".to_string()]);
        let snapshot = imported.seed_snapshot().unwrap();
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let auto: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            snapshot
                .variables
                .get(&(root, AUTO_MAP_VAR.to_string()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(auto["c1"], serde_json::Value::Bool(true));
        assert_eq!(auto["s11"], serde_json::Value::Bool(true));
        // A seeded chain step's vars land on its own scope; a nested node's
        // do not (no execution exists to carry them yet).
        assert!(
            snapshot
                .variables
                .contains_key(&(TaskId::new("s1", 0).unwrap(), "own".to_string())),
            "a seeded step's declared vars are durable facts"
        );
        assert!(
            !snapshot
                .variables
                .keys()
                .any(|(scope, _)| scope.node == "c1"),
            "a nested node's vars are seeded when its hop admits it"
        );

        // A step's `timeouts` import like its catches: the branch is dispatched
        // by the tick, which evaluates its guard, so both the branch and its
        // guard are durable model facts. Nothing seeds an execution for it —
        // the dispatch hop admits it.
        let with_timeout = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", crate::Vars::new())
                .with_timeout(|timeout| timeout.with_id("t1").with_if("$cost() >= 1000"))
        });
        let imported = spec(&with_timeout).unwrap();
        assert_eq!(imported.children, vec!["s1".to_string()]);
        let snapshot = imported.seed_snapshot().unwrap();
        assert!(
            snapshot
                .tasks
                .values()
                .all(|task| task.model_node.as_str() != "t1"),
            "the timer's hop admits the branch, not the seed"
        );
        // The branch's auto decision is a durable fact like a catch's.
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let timeout_auto: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            snapshot
                .variables
                .get(&(root, AUTO_MAP_VAR.to_string()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(timeout_auto["t1"], serde_json::Value::Bool(true));

        // A looping timeout branch needs step-loop machinery the kernel has
        // none of, like a looping handler.
        let looping_timeout = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", crate::Vars::new())
                .with_timeout(|timeout| timeout.with_id("t1").with_while("v < 3"))
        });
        let error = spec(&looping_timeout).unwrap_err();
        assert!(format!("{error}").contains("'while' guard"));

        // A step may declare `catches` beside `timeouts`: the legacy tree
        // builds them as Catch and Timeout children of the same node — the
        // failure path dispatches the handlers, the tick dispatches the
        // branches, and the step's join gates on every child it has while the
        // caught decision reads only the CATCH ones. Both sets import.
        let mixed_timeouts = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_catch(|catch| catch.with_id("c1"))
                .with_timeout(|timeout| timeout.with_id("t1"))
        });
        let imported = spec(&mixed_timeouts).unwrap();
        let snapshot = imported.seed_snapshot().unwrap();
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        let mixed_auto: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            snapshot
                .variables
                .get(&(root, AUTO_MAP_VAR.to_string()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(mixed_auto["c1"], serde_json::Value::Bool(true));
        assert_eq!(mixed_auto["t1"], serde_json::Value::Bool(true));

        // A step may declare `catches` beside `branches` too: the tree keeps
        // the two child sets apart — the branches are `Normal` children (they
        // run when the step completes) and the catches are `Catch` children
        // (only `Step::on_error` dispatches them) — so one step can carry both.
        let mixed = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("true")
                        .with_step(|step| step.with_id("s11"))
                })
                .with_catch(|step| step.with_id("c1"))
        });
        let imported = spec(&mixed).unwrap();
        let snapshot = imported.seed_snapshot().unwrap();
        assert!(
            snapshot.tasks.values().all(|task| {
                task.model_node.as_str() != "c1" && task.model_node.as_str() != "b1"
            }),
            "neither set is seeded: the branches fan out on completion, the handlers on failure"
        );
    }

    #[test]
    fn guarded_catch_steps_import_but_looping_ones_are_refused() {
        // A handler's `if` is its own selection (the legacy `Step::init`
        // guard, evaluated when the failed step dispatches its handlers), so
        // the import keeps it.
        let guarded = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", crate::Vars::new())
                .with_catch(|step| step.with_id("c1").with_if("$ecode() == 'err1'"))
        });
        let imported = spec(&guarded).unwrap();
        let snapshot = imported.seed_snapshot().unwrap();
        assert!(
            snapshot
                .tasks
                .values()
                .all(|task| task.model_node.as_str() != "c1"),
            "the handler still seeds no execution"
        );

        // A looping handler needs step-loop machinery the kernel has none of.
        let looping = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", crate::Vars::new())
                .with_catch(|step| step.with_id("c1").with_while("v < 3"))
        });
        let error = spec(&looping).unwrap_err();
        assert!(format!("{error}").contains("'while' guard"));
    }

    /// A guard inside a branch is imported: its start hop evaluates it, and a
    /// guard that fails falls through to the next step declared in the same
    /// branch (the skip carries `GUARD_SKIP_VAR`, so the join reads it as a
    /// fall-through rather than as the deadness cascade).
    #[test]
    fn guarded_steps_inside_branches_are_imported() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("true")
                    .with_step(|step| step.with_id("s11").with_if("v > 0"))
            })
        });
        let spec = spec(&workflow).unwrap();
        // The nested step's auto decision is seeded in the durable map (only
        // top-level steps join the seed set).
        let auto_map = spec
            .seed_variables
            .iter()
            .find(|(_, name, _)| name == AUTO_MAP_VAR)
            .map(|(_, _, value)| value.clone())
            .expect("the auto map is seeded");
        assert!(auto_map.contains("\"s11\":true"), "{auto_map}");
    }

    /// A `while` step cannot also declare a `next` (a while loop always flows
    /// to the next declared step when its condition fails), the same rule the
    /// model view enforces at build time.
    #[test]
    fn while_with_next_is_rejected() {
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("step1").with_next("step2").with_while("v > 0"));
        let error = spec(&workflow).unwrap_err();
        assert!(format!("{error}").contains("cannot combine 'while' with 'next'"));
    }

    #[test]
    fn top_level_guards_keep_the_flat_contract() {
        let workflow =
            crate::model::Workflow::new().with_step(|step| step.with_id("step1").with_if("v > 0"));
        let spec = spec(&workflow).unwrap();
        assert!(spec.auto.contains("step1"));
    }

    #[test]
    fn branches_beside_an_explicit_next_are_dead_config_like_legacy() {
        // legacy build: a step with `next` ignores its branches; so does the
        // import.
        let workflow = crate::model::Workflow::new()
            .with_step(|step| {
                step.with_id("step1")
                    .with_next("step2")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
            })
            .with_step(|step| step.with_id("step2"));
        let spec = spec(&workflow).unwrap();
        assert_eq!(spec.children.len(), 2);
        let snapshot = spec.seed_snapshot().unwrap();
        assert_eq!(snapshot.tasks.len(), 3);
    }

    /// The durable model of a run, decoded from the root scope's
    /// [`MODEL_VAR`] — what a hop reads the nested structure from.
    fn durable_model(snapshot: &ProcessSnapshot) -> crate::model::Workflow {
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        serde_json::from_str(
            snapshot
                .variables
                .get(&(root, MODEL_VAR.to_string()))
                .expect("the model is seeded on the root scope"),
        )
        .unwrap()
    }

    /// The auto map the import seeded, decoded.
    fn durable_auto(snapshot: &ProcessSnapshot) -> serde_json::Map<String, serde_json::Value> {
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        serde_json::from_str(
            snapshot
                .variables
                .get(&(root, AUTO_MAP_VAR.to_string()))
                .expect("the auto map is seeded on the root scope"),
        )
        .unwrap()
    }

    /// A container package's inline acts become the model's own shape, because
    /// the kernel's fan-out reads `branches`/`steps` and never a package's
    /// params: `acts.core.block` turns into an always-selected branch over its
    /// acts (one branch per act for `mode: parallel`), and
    /// `acts.core.parallel`/`sequence` into one always-selected branch holding
    /// the template step the run-time fan-out instantiates per `in` item. The
    /// template's own acts are materialized in turn, and every materialized
    /// step reaches the durable auto map (the fan-out reads its self-completion
    /// decision from there).
    #[test]
    fn container_package_acts_become_model_children() {
        let workflow = crate::model::Workflow::from_yml(
            r#"
name: containers
vars:
  - name: items
    value: ["u1", "u2"]
steps:
  - name: block
    id: block
    uses: acts.core.block
    params:
      mode: sequence
      acts:
        - uses: acts.core.msg
          params:
            key: m1
        - uses: acts.core.msg
          params:
            key: m2
  - name: fan
    id: fan
    uses: acts.core.parallel
    params:
      in: '${{ items }}'
      acts:
        - uses: acts.core.irq
          params:
            key: a1
"#,
        )
        .unwrap();
        let imported = ProcessSpec::from_workflow("p1", &workflow, &|uses| {
            // The `run_as` the registered core packages declare: what each
            // materialized node's auto decision is derived from.
            Some(match uses {
                "acts.core.msg" => crate::ActRunAs::Msg,
                "acts.core.irq" => crate::ActRunAs::Irq,
                _ => crate::ActRunAs::Func,
            })
        })
        .unwrap();
        let snapshot = imported.seed_snapshot().unwrap();
        let model = durable_model(&snapshot);
        let auto = durable_auto(&snapshot);

        // `acts.core.block{ mode: sequence }`: one always-selected branch over
        // both acts, in declaration order.
        let block = model.steps.iter().find(|step| step.id == "block").unwrap();
        assert_eq!(block.branches.len(), 1);
        let branch = &block.branches[0];
        assert_eq!(branch.id, "block#acts");
        assert_eq!(branch.r#if.as_deref(), Some("true"));
        assert_eq!(branch.steps.len(), 2);
        assert!(
            branch.steps.iter().all(|step| !step.id.is_empty()),
            "an unnamed act gets an id like any unnamed step: {:?}",
            branch.steps
        );

        // `acts.core.parallel{ in, acts }`: one always-selected branch holding
        // the template step, whose own acts are materialized as its branches.
        let fan = model.steps.iter().find(|step| step.id == "fan").unwrap();
        assert_eq!(fan.branches.len(), 1);
        assert_eq!(fan.branches[0].id, "fan#in");
        assert_eq!(fan.branches[0].r#if.as_deref(), Some("true"));
        assert_eq!(fan.branches[0].steps.len(), 1);
        let template = &fan.branches[0].steps[0];
        assert_eq!(template.id, "fan#item");
        assert_eq!(template.uses.as_deref(), Some(BLOCK_PACKAGE));
        assert_eq!(template.params["mode"], "parallel");
        assert_eq!(template.branches.len(), 1);
        assert_eq!(template.branches[0].id, "fan#item#act0");
        assert_eq!(
            template.branches[0].steps[0].uses.as_deref(),
            Some("acts.core.irq")
        );

        // Every materialized step carries an auto decision: the template is a
        // `Func` container (it self-completes and then fans out), the irq act
        // inside it waits for a client, and the block's msg acts self-complete
        // (the `run_as` their packages declare).
        assert_eq!(
            auto.get(&template.id),
            Some(&serde_json::Value::Bool(true)),
            "auto map must decide the template: {auto:?}"
        );
        assert_eq!(
            auto.get(&template.branches[0].steps[0].id),
            Some(&serde_json::Value::Bool(false)),
            "auto map must leave the irq act waiting: {auto:?}"
        );
        for step in &branch.steps {
            assert_eq!(auto.get(&step.id), Some(&serde_json::Value::Bool(true)));
        }
    }

    /// `mode: parallel` materializes one branch per act (the container admits
    /// its branches together), and a package the import does not know is left
    /// exactly as declared — a user package with an `acts` param is not a
    /// container.
    #[test]
    fn only_the_container_packages_are_materialized() {
        let workflow = crate::model::Workflow::from_yml(
            r#"
name: containers
steps:
  - name: parallel block
    id: pblock
    uses: acts.core.block
    params:
      mode: parallel
      acts:
        - uses: acts.core.msg
        - uses: acts.core.msg
  - name: foreign
    id: foreign
    uses: my.custom.package
    params:
      acts:
        - uses: acts.core.msg
"#,
        )
        .unwrap();
        let snapshot = spec(&workflow).unwrap().seed_snapshot().unwrap();
        let model = durable_model(&snapshot);

        let parallel = model.steps.iter().find(|step| step.id == "pblock").unwrap();
        assert_eq!(
            parallel
                .branches
                .iter()
                .map(|branch| branch.id.clone())
                .collect::<Vec<_>>(),
            vec!["pblock#act0".to_string(), "pblock#act1".to_string()]
        );
        assert!(
            parallel
                .branches
                .iter()
                .all(|branch| branch.steps.len() == 1),
            "one branch per act, one act per branch"
        );

        let foreign = model
            .steps
            .iter()
            .find(|step| step.id == "foreign")
            .unwrap();
        assert!(
            foreign.branches.is_empty(),
            "a package the import does not know keeps its declaration"
        );
    }
}
