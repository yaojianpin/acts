//! The run's graph (`proc:graph`): the model's declaration expanded over the
//! aggregate's executions — every node the durable model holds, executed or
//! not, plus one node per execution instance the run admitted.
//!
//! A message names the model node it ran on (`nid`) and one execution instance
//! (`tid`); the read model's rows exist only for executions. The declaration
//! supplies the shape and the ids, the aggregate supplies who actually ran:
//! a fan-out's template appears once per generation (the `in` items share one
//! model node), and an execution the model does not declare — a client's
//! pushed act, the delegated join's request — appears under the execution that
//! created it, with its facts read from the child scope. A node that never ran
//! has no `tid` and no `state`: the flow ahead of a waiting act.
//!
//! The walk reads the durable model (the copy the import seeds, inline
//! container acts already materialized as `<step>#acts`/`<step>#item`/
//! `<step>#act<N>` children), so the ids are the ones the run's messages carry.
//! The materialized container is the kernel's own branch — the declaration
//! declares no branch — so it is kinded `block`, and the graph's tree display
//! folds it into its parent step.

use super::*;
use crate::scheduler::core::commit::aggregate::ProcessSnapshot;
use crate::scheduler::core::model::task_graph::{TaskId, TaskState};
use std::collections::{HashMap, HashSet};

impl KernelHost {
    /// The run's graph with its execution instances and states.
    pub(crate) async fn graph(&self, pid: &str) -> Result<crate::ProcessGraph> {
        let snapshot = self
            .snapshot(pid)
            .await?
            .ok_or_else(|| runtime(format!("aggregate '{pid}' not found")))?;
        let model = facts::model_of(&snapshot)?
            .ok_or_else(|| runtime(format!("aggregate '{pid}' carries no model")))?;
        process_graph(&snapshot, &model)
    }
}

/// The declaration and the executions one graph walk reads: every execution by
/// model node (in generation order), and the set of node ids the model
/// declares — the split between a declared execution and a dynamic one (a
/// pushed act, a join request).
struct GraphContext<'a> {
    snapshot: &'a ProcessSnapshot,
    executions: HashMap<String, Vec<&'a TaskId>>,
    declared: HashSet<String>,
}

impl<'a> GraphContext<'a> {
    fn new(snapshot: &'a ProcessSnapshot, model: &crate::model::Workflow) -> Self {
        let mut executions: HashMap<String, Vec<&'a TaskId>> = HashMap::new();
        for id in snapshot.tasks.keys() {
            executions
                .entry(id.node.as_str().to_owned())
                .or_default()
                .push(id);
        }
        for ids in executions.values_mut() {
            ids.sort_by_key(|id| id.generation);
        }
        let mut declared = HashSet::new();
        declared.insert(crate::utils::consts::TASK_ROOT_TID.to_string());
        for step in &model.steps {
            collect_declared(step, &mut declared);
        }
        Self {
            snapshot,
            executions,
            declared,
        }
    }

    /// The executions of `nid` whose parent is `parent`, oldest generation
    /// first. The parent is the execution that admitted them, so the same
    /// model node under two fan-out items stays two lists.
    fn executions_under(&self, nid: &str, parent: Option<&TaskId>) -> Vec<&'a TaskId> {
        self.executions
            .get(nid)
            .map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| {
                        self.snapshot
                            .tasks
                            .get(*id)
                            .is_some_and(|task| task.parent.as_ref() == parent)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The children of one execution the model does not declare, in a stable
    /// order: a client's pushed acts and the delegated join's requests. The
    /// walk descends into each (a push may sit under a pushed act).
    fn dynamic_children(&self, parent: &TaskId) -> Vec<&'a TaskId> {
        let mut children: Vec<&TaskId> = self
            .snapshot
            .tasks
            .iter()
            .filter(|(_, task)| {
                task.parent.as_ref() == Some(parent)
                    && !self.declared.contains(task.model_node.as_str())
            })
            .map(|(id, _)| id)
            .collect();
        children.sort_by(|left, right| {
            left.node
                .as_str()
                .cmp(right.node.as_str())
                .then_with(|| left.generation.cmp(&right.generation))
        });
        children
    }
}

fn collect_declared(step: &crate::model::Step, declared: &mut HashSet<String>) {
    declared.insert(step.id.clone());
    for branch in super::model::step_branches(step).unwrap_or_default() {
        declared.insert(branch.id.clone());
        for inner in &branch.steps {
            collect_declared(inner, declared);
        }
    }
    for catch in &step.catches {
        collect_declared(catch, declared);
    }
    for timeout in &step.timeouts {
        collect_declared(timeout, declared);
    }
}

/// One emitted node's declaration facts. The execution facts (`tid`, state,
/// generation) come from the aggregate; the node is either one execution of
/// this declaration or the declaration itself when nothing ran.
#[derive(Clone)]
struct NodeSpec {
    nid: String,
    name: String,
    kind: String,
    uses: Option<String>,
    parent: Option<String>,
    next: Option<String>,
}

fn push_execution(
    nodes: &mut Vec<crate::GraphNode>,
    context: &GraphContext<'_>,
    spec: NodeSpec,
    tid: &TaskId,
) {
    let task = context
        .snapshot
        .tasks
        .get(tid)
        .expect("graph executions come from the snapshot");
    // A seeded `Created` execution is still the flow ahead, exactly like a
    // node with no execution: it has no state to show.
    let live = (!matches!(task.state, TaskState::Created)).then_some(task.state);
    nodes.push(crate::GraphNode {
        nid: spec.nid,
        tid: Some(tid.wire(&context.snapshot.process_id)),
        name: spec.name,
        kind: spec.kind,
        uses: spec.uses,
        parent: spec.parent,
        parent_tid: task
            .parent
            .as_ref()
            .map(|parent| parent.wire(&context.snapshot.process_id)),
        order: nodes.len(),
        next: spec.next,
        state: live.map(|state| state.as_str().to_string()),
        generation: live.map(|_| tid.generation),
    });
}

fn push_placeholder(
    nodes: &mut Vec<crate::GraphNode>,
    spec: NodeSpec,
    parent_tid: Option<&TaskId>,
    process_id: &str,
) {
    nodes.push(crate::GraphNode {
        nid: spec.nid,
        tid: None,
        name: spec.name,
        kind: spec.kind,
        uses: spec.uses,
        parent: spec.parent,
        parent_tid: parent_tid.map(|parent| parent.wire(process_id)),
        order: nodes.len(),
        next: spec.next,
        state: None,
        generation: None,
    });
}

/// Walk one materialized model into its node list, expanding each node over
/// the executions the aggregate holds for it.
pub(crate) fn process_graph(
    snapshot: &ProcessSnapshot,
    model: &crate::model::Workflow,
) -> Result<crate::ProcessGraph> {
    let context = GraphContext::new(snapshot, model);
    let mut nodes = Vec::new();
    // The workflow root is a node like any other: the run's own lifecycle
    // messages name it.
    let root = crate::utils::consts::TASK_ROOT_TID;
    let root_spec = NodeSpec {
        nid: root.to_string(),
        name: model.name.clone(),
        kind: crate::NodeKind::Workflow.to_string(),
        uses: None,
        parent: None,
        next: None,
    };
    let roots = context.executions_under(root, None);
    if roots.is_empty() {
        push_placeholder(&mut nodes, root_spec, None, &snapshot.process_id);
        for step in &model.steps {
            walk_step(&mut nodes, &context, step, root, None, true)?;
        }
    } else {
        let repeated = roots.len() > 1;
        for execution in roots {
            push_execution(&mut nodes, &context, root_spec.clone(), execution);
            for step in &model.steps {
                walk_step(&mut nodes, &context, step, root, Some(execution), !repeated)?;
            }
            for dynamic in context.dynamic_children(execution) {
                walk_dynamic(&mut nodes, &context, dynamic)?;
            }
        }
    }
    Ok(crate::ProcessGraph {
        pid: snapshot.process_id.clone(),
        mid: model.id.clone(),
        nodes,
    })
}

/// One step and everything it declares: its branches (each a node over its own
/// steps), then its catch handlers and timeout branches — the shape the
/// import's materialized copy carries. Each node emits one graph node per
/// execution under `parent_tid`; a node without executions is emitted once,
/// with no state, when its parent is not a fan-out (a fan-out's items only
/// show the executions they actually admitted).
fn walk_step(
    nodes: &mut Vec<crate::GraphNode>,
    context: &GraphContext<'_>,
    step: &crate::model::Step,
    model_parent: &str,
    parent_tid: Option<&TaskId>,
    show_unreached: bool,
) -> Result<()> {
    let spec = NodeSpec {
        nid: step.id.clone(),
        name: step.name.clone(),
        kind: crate::NodeKind::Step.to_string(),
        uses: step.uses.clone().filter(|uses| !uses.trim().is_empty()),
        parent: Some(model_parent.to_string()),
        next: step.next.clone(),
    };
    let executions = context.executions_under(&step.id, parent_tid);
    if executions.is_empty() {
        if !show_unreached {
            return Ok(());
        }
        push_placeholder(nodes, spec, parent_tid, &context.snapshot.process_id);
        for branch in super::model::step_branches(step).unwrap_or_default() {
            walk_branch(nodes, context, branch, &step.id, parent_tid, true)?;
        }
        for catch in &step.catches {
            walk_step(nodes, context, catch, &step.id, parent_tid, true)?;
        }
        for timeout in &step.timeouts {
            walk_step(nodes, context, timeout, &step.id, parent_tid, true)?;
        }
        return Ok(());
    }
    let repeated = executions.len() > 1;
    for execution in executions {
        push_execution(nodes, context, spec.clone(), execution);
        // The declaration's own shape under this execution. A fan-out's
        // repeated template shows only the children it actually admitted.
        for branch in super::model::step_branches(step).unwrap_or_default() {
            walk_branch(nodes, context, branch, &step.id, Some(execution), !repeated)?;
        }
        for catch in &step.catches {
            walk_step(nodes, context, catch, &step.id, Some(execution), !repeated)?;
        }
        for timeout in &step.timeouts {
            walk_step(
                nodes,
                context,
                timeout,
                &step.id,
                Some(execution),
                !repeated,
            )?;
        }
        for dynamic in context.dynamic_children(execution) {
            walk_dynamic(nodes, context, dynamic)?;
        }
    }
    Ok(())
}

fn walk_branch(
    nodes: &mut Vec<crate::GraphNode>,
    context: &GraphContext<'_>,
    branch: &crate::model::Branch,
    model_parent: &str,
    parent_tid: Option<&TaskId>,
    show_unreached: bool,
) -> Result<()> {
    let kind = if branch.r#virtual {
        crate::NodeKind::Block
    } else {
        crate::NodeKind::Branch
    };
    let spec = NodeSpec {
        nid: branch.id.clone(),
        name: branch.name.clone(),
        kind: kind.to_string(),
        uses: None,
        parent: Some(model_parent.to_string()),
        next: None,
    };
    let executions = context.executions_under(&branch.id, parent_tid);
    if executions.is_empty() {
        if !show_unreached {
            return Ok(());
        }
        push_placeholder(nodes, spec, parent_tid, &context.snapshot.process_id);
        for inner in &branch.steps {
            walk_step(nodes, context, inner, &branch.id, parent_tid, true)?;
        }
        return Ok(());
    }
    let repeated = executions.len() > 1;
    for execution in executions {
        push_execution(nodes, context, spec.clone(), execution);
        for inner in &branch.steps {
            walk_step(
                nodes,
                context,
                inner,
                &branch.id,
                Some(execution),
                !repeated,
            )?;
        }
        for dynamic in context.dynamic_children(execution) {
            walk_dynamic(nodes, context, dynamic)?;
        }
    }
    Ok(())
}

/// One execution the model does not declare: a pushed act or a delegated
/// join's request. Its facts ride the child scope (the model is immutable for
/// the run's life), so the graph reads them back from there.
fn walk_dynamic(
    nodes: &mut Vec<crate::GraphNode>,
    context: &GraphContext<'_>,
    tid: &TaskId,
) -> Result<()> {
    let parent = context
        .snapshot
        .tasks
        .get(tid)
        .and_then(|task| task.parent.as_ref())
        .ok_or_else(|| runtime(format!("dynamic execution {tid:?} has no parent")))?;
    let act = super::action::pushed_act_of(context.snapshot, tid)?;
    let (name, uses) = match act {
        Some(act) => (act.name, Some(act.uses)),
        None => (String::new(), None),
    };
    let spec = NodeSpec {
        nid: tid.node.as_str().to_owned(),
        name,
        kind: crate::NodeKind::Step.to_string(),
        uses,
        parent: Some(parent.node.as_str().to_owned()),
        next: None,
    };
    push_execution(nodes, context, spec, tid);
    for child in context.dynamic_children(tid) {
        walk_dynamic(nodes, context, child)?;
    }
    Ok(())
}
