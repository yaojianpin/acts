//! The run's declared model graph (`proc:graph`): every node the durable
//! (materialized) model holds, in declaration order, with each node's newest
//! execution state folded in — the static and the live half of a client's flow
//! trace on one node list.
//!
//! A message names the model node it ran on (`nid`) and one execution
//! instance (`tid`); the read model's rows exist only for executions. The
//! nodes no execution reached — the flow ahead of a waiting act, a branch the
//! run has not selected yet — have no `state`, while a fan-out node reports its
//! newest generation (per-instance detail stays in the rows and messages). The
//! walk reads the durable model (the copy the import seeds, inline container
//! acts already materialized as `<step>#acts`/`<step>#item`/`<step>#act<N>`
//! children), so the ids are the ones the run's messages carry.

use super::*;
use crate::scheduler::core::model::task_graph::TaskState;
use std::collections::HashMap;

impl KernelHost {
    /// The run's declared model graph with its execution states.
    pub(crate) async fn graph(&self, pid: &str) -> Result<crate::ProcessGraph> {
        let snapshot = self
            .snapshot(pid)
            .await?
            .ok_or_else(|| runtime(format!("aggregate '{pid}' not found")))?;
        let model = facts::model_of(&snapshot)?
            .ok_or_else(|| runtime(format!("aggregate '{pid}' carries no model")))?;
        Ok(process_graph(&snapshot, &model))
    }
}

/// One node's newest execution: the generation and its state.
type Newest = HashMap<String, (u64, TaskState)>;

/// Walk one materialized model into its node list, folding each node's newest
/// execution state in.
pub(crate) fn process_graph(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    model: &crate::model::Workflow,
) -> crate::ProcessGraph {
    // The aggregate is the truth; the read model's rows lag it. One pass keeps
    // the newest generation per node (a fan-out item and a `while` turn are
    // generations of the same node).
    let mut newest: Newest = HashMap::new();
    for (id, task) in &snapshot.tasks {
        let entry = newest
            .entry(id.node.as_str().to_owned())
            .or_insert((id.generation, task.state));
        if id.generation >= entry.0 {
            *entry = (id.generation, task.state);
        }
    }

    let mut nodes = Vec::new();
    // The workflow root is a node like any other: the run's own lifecycle
    // messages name it.
    let root = crate::utils::consts::TASK_ROOT_TID;
    let facts = facts::node_facts(model, root);
    nodes.push(crate::GraphNode {
        nid: root.to_string(),
        name: facts.name,
        kind: facts.declared_kind.to_string(),
        uses: None,
        parent: None,
        order: 0,
        next: None,
        state: state_of(&newest, root),
        generation: generation_of(&newest, root),
    });
    for step in &model.steps {
        walk_step(&mut nodes, &newest, model, step, root);
    }
    crate::ProcessGraph {
        pid: snapshot.process_id.clone(),
        mid: model.id.clone(),
        nodes,
    }
}

/// One step and everything it declares: its branches (each a node over its own
/// steps), then its catch handlers and timeout branches — the shape the
/// import's materialized copy carries.
fn walk_step(
    nodes: &mut Vec<crate::GraphNode>,
    newest: &Newest,
    model: &crate::model::Workflow,
    step: &crate::model::Step,
    parent: &str,
) {
    let facts = facts::node_facts(model, &step.id);
    nodes.push(crate::GraphNode {
        nid: step.id.clone(),
        name: if facts.name.is_empty() {
            step.name.clone()
        } else {
            facts.name
        },
        kind: facts.declared_kind.to_string(),
        uses: step.uses.clone().filter(|uses| !uses.trim().is_empty()),
        parent: Some(parent.to_string()),
        order: nodes.len(),
        next: step.next.clone(),
        state: state_of(newest, &step.id),
        generation: generation_of(newest, &step.id),
    });
    // `step_branches` mirrors the tree build: a step declaring an explicit
    // `next` ignores its branches, so the graph does too.
    for branch in super::model::step_branches(step).unwrap_or_default() {
        nodes.push(crate::GraphNode {
            nid: branch.id.clone(),
            name: branch.name.clone(),
            kind: crate::NodeKind::Branch.to_string(),
            uses: None,
            parent: Some(step.id.clone()),
            order: nodes.len(),
            next: None,
            state: state_of(newest, &branch.id),
            generation: generation_of(newest, &branch.id),
        });
        for inner in &branch.steps {
            walk_step(nodes, newest, model, inner, &branch.id);
        }
    }
    for catch in &step.catches {
        walk_step(nodes, newest, model, catch, &step.id);
    }
    for timeout in &step.timeouts {
        walk_step(nodes, newest, model, timeout, &step.id);
    }
}

/// A node's newest state in the read model's vocabulary (the rows' words), or
/// `None` when no execution has reached it: a node the seed pre-created is
/// still `Created` — the flow has not woken it — so it has no state to show
/// (and no generation), exactly like a node with no execution at all.
fn state_of(newest: &Newest, nid: &str) -> Option<String> {
    newest
        .get(nid)
        .filter(|(_, state)| !matches!(state, TaskState::Created))
        .map(|(_, state)| state.as_str().to_string())
}

fn generation_of(newest: &Newest, nid: &str) -> Option<u64> {
    newest
        .get(nid)
        .filter(|(_, state)| !matches!(state, TaskState::Created))
        .map(|(generation, _)| *generation)
}
