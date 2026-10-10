//! Declarative process specification for the kernel host.
//!
//! P1 seeds a flat workflow: one root and a declared child chain with optional
//! explicit join edges. The same two builders (model graph, execution graph)
//! carry the real model import in P2, so the vocabulary here stays exactly
//! `kernel::ModelGraph`/`kernel::ExecutionGraph` — no side structures.

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

/// A flat process: `root` with `children` in declaration order (implicit
/// successor chain) plus optional explicit edges, e.g. a join that both
/// branches point at. Every node becomes one execution at generation 0.
///
/// `auto` names the tasks whose execution self-completes (no waiting
/// package); every other task stays `Running` until
/// [`KernelHost::apply_action`] applies a client action. The set is seeded
/// into the aggregate as the per-execution `auto` variable, so recovery
/// replays never need the declaring spec.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessSpec {
    pub(crate) process_id: String,
    pub(crate) root: String,
    pub(crate) children: Vec<String>,
    pub(crate) explicit_next: Vec<(String, String)>,
    pub(crate) auto: std::collections::BTreeSet<String>,
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

impl ProcessSpec {
    pub(crate) fn linear(process_id: impl Into<String>, root: &str, children: &[&str]) -> Self {
        Self {
            process_id: process_id.into(),
            root: root.to_string(),
            children: children.iter().map(|name| name.to_string()).collect(),
            explicit_next: Vec::new(),
            auto: children.iter().map(|name| name.to_string()).collect(),
        }
    }

    /// Import a flat workflow: the legacy root task id (`$`) as root and the
    /// workflow's top-level steps as one chain. Nesting (branches) needs P2's
    /// `ExecutionCreate` import and is rejected here instead of being
    /// silently flattened into wrong semantics.
    pub(crate) fn from_workflow(
        process_id: impl Into<String>,
        workflow: &crate::model::Workflow,
        run_as: &dyn Fn(&str) -> Option<crate::ActRunAs>,
    ) -> Result<Self> {
        for step in &workflow.steps {
            if !step.branches.is_empty() {
                return Err(ActError::Runtime(format!(
                    "step '{}' has branches; nested models are not imported yet",
                    step.id
                )));
            }
        }
        let auto = workflow
            .steps
            .iter()
            .filter(|step| is_auto(step.uses.as_deref(), run_as))
            .map(|step| step.id.clone())
            .collect();
        Ok(Self {
            process_id: process_id.into(),
            root: crate::utils::consts::TASK_ROOT_TID.to_string(),
            children: workflow.steps.iter().map(|step| step.id.clone()).collect(),
            explicit_next: Vec::new(),
            auto,
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
