//! Declarative process specification for the kernel host.
//!
//! P1 seeded a flat workflow: one root and a declared child chain with
//! optional explicit join edges. The nested import (P2) keeps the same
//! vocabulary — `kernel::ModelGraph`/`kernel::ExecutionGraph` — for the
//! seeded top chain, and carries the nested structure (branch subtrees) as
//! durable aggregate variables (`__model`, `__auto`): the fan-out hop reads
//! them from the snapshot, so recovery replays never need the declaring
//! spec. Branch executions and their inner chains come alive through
//! `kernel::ContainerPlanner` fan-outs, never at seed time.

use crate::kernel::{
    AggregateSnapshot, ChildRole, ExecutionGraph, ExecutionGraphBuilder, ExecutionId,
    ExecutionNode, ExecutionSnapshot, ExecutionState, ModelGraph, ModelGraphBuilder,
    NodeDefinition, NodeId, NodeKind, OperationId, OperationKind, OperationRecord,
};
use crate::{ActError, Result};

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

/// One step of the nested import walk: pin the step's auto decision and
/// declared vars, reject what the shadow cannot mean faithfully, and descend
/// into its branches. `top_level` steps keep the P1 contract (their `if` /
/// `while` guards were already ignored there); guards inside a branch would
/// silently change meaning, so they are refused.
fn import_step(
    step: &crate::model::Step,
    run_as: &dyn Fn(&str) -> Option<crate::ActRunAs>,
    top_level: bool,
    auto: &mut std::collections::BTreeSet<String>,
    auto_map: &mut serde_json::Map<String, serde_json::Value>,
    seed_variables: &mut Vec<(Option<String>, String, String)>,
) -> Result<()> {
    let step_auto = is_auto(step.uses.as_deref(), run_as);
    auto_map.insert(step.id.clone(), serde_json::Value::Bool(step_auto));
    if top_level && step_auto {
        auto.insert(step.id.clone());
    }
    for var in &step.vars {
        seed_variables.push((
            Some(step.id.clone()),
            var.name.clone(),
            var.value.to_string(),
        ));
    }
    if !top_level && (step.r#if.is_some() || step.r#while.is_some()) {
        return Err(ActError::Runtime(format!(
            "step '{}' carries an 'if'/'while' guard inside a branch; guarded nested steps are not imported yet",
            step.id
        )));
    }
    for branch in step_branches(step).unwrap_or_default() {
        if !branch.needs.is_empty() {
            return Err(ActError::Runtime(format!(
                "branch '{}' declares 'needs'; branch dependencies are not imported yet",
                branch.id
            )));
        }
        for inner in &branch.steps {
            import_step(inner, run_as, false, auto, auto_map, seed_variables)?;
        }
    }
    Ok(())
}

impl ProcessSpec {
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
    /// re-evaluation driver for yet) and `if`/`while` guards on steps inside
    /// a branch (the kernel join treats `Skipped` as a dead path, while a
    /// legacy step guard falls through to the chain — one state cannot mean
    /// both).
    pub(crate) fn from_workflow(
        process_id: impl Into<String>,
        workflow: &crate::model::Workflow,
        run_as: &dyn Fn(&str) -> Option<crate::ActRunAs>,
    ) -> Result<Self> {
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
                true,
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
                    kind: NodeKind::Act,
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
    pub(crate) fn seed_snapshot(&self) -> Result<AggregateSnapshot> {
        self.model_graph()?;
        let mut snapshot = AggregateSnapshot::empty(&self.process_id).map_err(runtime)?;
        snapshot.version.graph = 1;

        let root = ExecutionId::new(&self.root, 0).map_err(runtime)?;
        let mut executions = std::collections::HashMap::new();
        executions.insert(
            root.clone(),
            ExecutionSnapshot {
                model_node: node_id(&self.root)?,
                parent: None,
                predecessors: Vec::new(),
                state: ExecutionState::Ready,
                revision: 0,
            },
        );
        for child in &self.children {
            let id = ExecutionId::new(child, 0).map_err(runtime)?;
            executions.insert(
                id,
                ExecutionSnapshot {
                    model_node: node_id(child)?,
                    parent: Some(root.clone()),
                    predecessors: Vec::new(),
                    state: ExecutionState::Created,
                    revision: 0,
                },
            );
        }
        snapshot.executions = executions;
        // The auto/waiting decision is durable state, not spec state: the
        // aggregate replays without the model.
        for id in snapshot.executions.keys() {
            let auto = id.task_id == self.root || self.auto.contains(&id.task_id);
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
                Some(node) => ExecutionId::new(node, 0).map_err(runtime)?,
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
        // At creation the MODEL is the edge authority: every node's successor
        // becomes an execution edge, and the predecessor lists are its mirror.
        // From then on the snapshot itself is the graph of record.
        let model = self.model_graph()?;
        let by_node = |node: &NodeId| {
            snapshot
                .executions
                .iter()
                .find(|(_, candidate)| &candidate.model_node == node)
                .map(|(id, _)| id.clone())
        };
        let mut edges: Vec<(ExecutionId, ExecutionId)> = Vec::new();
        for (id, execution) in &snapshot.executions {
            if let Some(successor) = model.successor(&execution.model_node) {
                let Some(target) = by_node(successor) else {
                    return Err(ActError::Runtime("successor without execution".into()));
                };
                edges.push((id.clone(), target));
            }
        }
        for (from, to) in edges {
            if let Some(target) = snapshot.executions.get_mut(&to) {
                target.predecessors.push(from);
            }
        }

        let operation_id = OperationId {
            process_id: self.process_id.clone(),
            task_id: self.root.clone(),
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
/// and predecessor lists in `ExecutionSnapshot` are the graph of record,
/// seeded from the model at creation and extended by every
/// `ExecutionCreate`. The root is the single parentless execution; the
/// revision is the aggregate's graph revision, which is what
/// `PropagationPlanner` checks.
pub(crate) fn execution_graph(snapshot: &AggregateSnapshot) -> Result<ExecutionGraph> {
    let mut builder = ExecutionGraphBuilder::new(&snapshot.process_id, snapshot.version.graph)
        .map_err(runtime)?;
    let mut root: Option<ExecutionId> = None;
    for (id, execution) in &snapshot.executions {
        builder
            .add_node(ExecutionNode {
                id: id.clone(),
                model_node: execution.model_node.clone(),
                state: execution.state,
                revision: execution.revision,
            })
            .map_err(runtime)?;
        match &execution.parent {
            Some(parent) => builder
                .add_child(parent.clone(), id.clone())
                .map_err(runtime)?,
            None => root = Some(id.clone()),
        }
    }
    let root = root.ok_or_else(|| ActError::Runtime("no root execution".into()))?;
    builder.set_root(root).map_err(runtime)?;
    for (id, execution) in &snapshot.executions {
        for predecessor in &execution.predecessors {
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
        let root = ExecutionId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
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
        let root = ExecutionId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
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
        assert_eq!(snapshot.executions.len(), 2, "root + step1 only");
        // The root scope carries the workflow vars, the model and the auto
        // map — the fan-out's durable inputs.
        let root = ExecutionId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
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
    fn guarded_steps_inside_branches_are_rejected() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("true")
                    .with_step(|step| step.with_id("s11").with_if("v > 0"))
            })
        });
        let error = spec(&workflow).unwrap_err();
        assert!(format!("{error}").contains("'if'/'while' guard inside a branch"));
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
        assert_eq!(snapshot.executions.len(), 3);
    }
}
