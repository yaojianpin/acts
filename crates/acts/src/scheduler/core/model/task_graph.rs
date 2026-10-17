use crate::scheduler::core::model::model_graph::NodeId;
use std::collections::{HashMap, HashSet};
use std::fmt;

/// The identity of one instance of a model node: the node's `generation`-th
/// run. Deterministic and a pure function of its parts — idempotency (whole
/// snapshot equality), the canonical digest and CAS retries all depend on it.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TaskId {
    pub(crate) node: NodeId,
    pub(crate) generation: u64,
}

impl TaskId {
    pub(crate) fn new(node: impl Into<String>, generation: u64) -> Result<Self, TaskGraphError> {
        let node = NodeId::parse(node).map_err(|_| TaskGraphError::EmptyTaskId)?;
        Ok(Self { node, generation })
    }

    /// The single derivation of the identity's wire form: the instance's
    /// external id (`tid` — read-model row keys, message `tid`, the action
    /// address). D2 (design §2.2): the irreversible fixed-length digest
    /// `sha256(pid ‖ 0x00 ‖ node ‖ 0x00 ‖ gen_le)[..32]` — constant length
    /// (row/index keys never grow with the node's name), injective over
    /// `(node, generation)` within a process, a pure function of its parts
    /// (idempotent whole-snapshot equality, the canonical digest and CAS
    /// retries keep working), and not decodable back without the aggregate —
    /// which is why every reader that needs the *node* holds a `TaskId` (or a
    /// row's `nid`), never a reverse lookup.
    pub(crate) fn wire(&self, process_id: &str) -> String {
        let mut bytes = Vec::with_capacity(process_id.len() + self.node.as_str().len() + 9);
        bytes.extend_from_slice(process_id.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(self.node.as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        crate::scheduler::core::commit::aggregate::sha256_hex(&bytes)[..32].to_owned()
    }
}

impl fmt::Debug for TaskId {
    /// Renders the identity flat — the node string, not the newtype — so
    /// diagnostics that interpolate an id (`execution {task_id:?} not found`)
    /// keep their historical byte shape.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskId")
            .field("node", &self.node.as_str())
            .field("generation", &self.generation)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TaskState {
    Created,
    Ready,
    Running,
    Waiting,
    Completed,
    Submitted,
    Backed,
    Cancelled,
    Failed,
    Aborted,
    Skipped,
    Removed,
}

impl TaskState {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Submitted
                | Self::Backed
                | Self::Cancelled
                | Self::Failed
                | Self::Aborted
                | Self::Skipped
                | Self::Removed
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskNode {
    pub(crate) id: TaskId,
    pub(crate) model_node: NodeId,
    pub(crate) state: TaskState,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskGraph {
    pub(crate) process_id: String,
    pub(crate) revision: u64,
    nodes: HashMap<TaskId, TaskNode>,
    children: HashMap<TaskId, Vec<TaskId>>,
    predecessors: HashMap<TaskId, Vec<TaskId>>,
    successors: HashMap<TaskId, Vec<TaskId>>,
}

impl TaskGraph {
    pub(crate) fn node(&self, id: &TaskId) -> Option<&TaskNode> {
        self.nodes.get(id)
    }

    pub(crate) fn children(&self, id: &TaskId) -> &[TaskId] {
        self.children.get(id).map(Vec::as_slice).unwrap_or_default()
    }
    pub(crate) fn predecessors(&self, id: &TaskId) -> &[TaskId] {
        self.predecessors
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    pub(crate) fn successors(&self, id: &TaskId) -> &[TaskId] {
        self.successors
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

pub(crate) struct TaskGraphBuilder {
    process_id: String,
    revision: u64,
    root: Option<TaskId>,
    nodes: HashMap<TaskId, TaskNode>,
    parent: HashMap<TaskId, TaskId>,
    children: HashMap<TaskId, Vec<TaskId>>,
    predecessors: HashMap<TaskId, Vec<TaskId>>,
    successors: HashMap<TaskId, Vec<TaskId>>,
}

impl TaskGraphBuilder {
    pub(crate) fn new(
        process_id: impl Into<String>,
        revision: u64,
    ) -> Result<Self, TaskGraphError> {
        let process_id = process_id.into();
        if process_id.trim().is_empty() {
            return Err(TaskGraphError::EmptyProcessId);
        }
        Ok(Self {
            process_id,
            revision,
            root: None,
            nodes: HashMap::new(),
            parent: HashMap::new(),
            children: HashMap::new(),
            predecessors: HashMap::new(),
            successors: HashMap::new(),
        })
    }

    pub(crate) fn add_node(&mut self, node: TaskNode) -> Result<(), TaskGraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(TaskGraphError::DuplicateTask(node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub(crate) fn set_root(&mut self, id: TaskId) -> Result<(), TaskGraphError> {
        if self.root.replace(id).is_some() {
            return Err(TaskGraphError::MultipleRoots);
        }
        Ok(())
    }

    pub(crate) fn add_child(
        &mut self,
        parent: TaskId,
        child: TaskId,
    ) -> Result<(), TaskGraphError> {
        if self.parent.contains_key(&child) {
            return Err(TaskGraphError::MultipleParents(child));
        }
        self.parent.insert(child.clone(), parent.clone());
        self.children.entry(parent).or_default().push(child);
        Ok(())
    }

    pub(crate) fn add_successor(&mut self, from: TaskId, to: TaskId) -> Result<(), TaskGraphError> {
        if self
            .successors
            .get(&from)
            .is_some_and(|items| items.contains(&to))
        {
            return Err(TaskGraphError::DuplicateSuccessor { from, to });
        }
        self.predecessors
            .entry(to.clone())
            .or_default()
            .push(from.clone());
        self.successors.entry(from).or_default().push(to);
        Ok(())
    }

    pub(crate) fn build(self) -> Result<TaskGraph, TaskGraphError> {
        let root = self.root.clone().ok_or(TaskGraphError::MissingRoot)?;
        self.require_node(&root)?;
        if self.parent.contains_key(&root) {
            return Err(TaskGraphError::RootHasParent);
        }
        for (child, parent) in &self.parent {
            self.require_node(child)?;
            self.require_node(parent)?;
        }
        for (next, prevs) in &self.predecessors {
            self.require_node(next)?;
            for prev in prevs {
                self.require_node(prev)?;
            }
        }

        let mut seen = HashSet::new();
        let mut stack = vec![root.clone()];
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(children) = self.children.get(&id) {
                stack.extend(children.iter().cloned());
            }
        }
        if seen.len() != self.nodes.len() {
            let mut ids = self
                .nodes
                .keys()
                .filter(|id| !seen.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            ids.sort();
            return Err(TaskGraphError::UnreachableTasks(ids));
        }

        Ok(TaskGraph {
            process_id: self.process_id,
            revision: self.revision,
            nodes: self.nodes,
            children: self.children,
            predecessors: self.predecessors,
            successors: self.successors,
        })
    }

    fn require_node(&self, id: &TaskId) -> Result<(), TaskGraphError> {
        if self.nodes.contains_key(id) {
            Ok(())
        } else {
            Err(TaskGraphError::UnknownTask(id.clone()))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TaskGraphError {
    EmptyProcessId,
    EmptyTaskId,
    MissingRoot,
    MultipleRoots,
    RootHasParent,
    DuplicateTask(TaskId),
    UnknownTask(TaskId),
    MultipleParents(TaskId),
    DuplicateSuccessor { from: TaskId, to: TaskId },
    UnreachableTasks(Vec<TaskId>),
}
impl fmt::Display for TaskGraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "execution graph error: {self:?}")
    }
}
impl std::error::Error for TaskGraphError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn eid(id: &str, generation: u64) -> TaskId {
        TaskId::new(id, generation).unwrap()
    }
    fn node(id: &str, generation: u64) -> TaskNode {
        TaskNode {
            id: eid(id, generation),
            model_node: NodeId::parse(id).unwrap(),
            state: TaskState::Created,
            revision: 0,
        }
    }

    #[test]
    fn generations_are_distinct_task_identity() {
        let mut b = TaskGraphBuilder::new("p1", 4).unwrap();
        for n in [node("root", 0), node("step", 0), node("step", 1)] {
            b.add_node(n).unwrap();
        }
        b.set_root(eid("root", 0)).unwrap();
        b.add_child(eid("root", 0), eid("step", 0)).unwrap();
        b.add_child(eid("root", 0), eid("step", 1)).unwrap();
        let graph = b.build().unwrap();
        assert_eq!(
            graph.children(&eid("root", 0)),
            &[eid("step", 0), eid("step", 1)]
        );
    }

    #[test]
    fn join_accepts_multiple_predecessors_but_rejects_duplicate_edge() {
        let mut b = TaskGraphBuilder::new("p1", 0).unwrap();
        for n in [node("root", 0), node("a", 0), node("b", 0)] {
            b.add_node(n).unwrap();
        }
        b.add_successor(eid("root", 0), eid("b", 0)).unwrap();
        b.add_successor(eid("a", 0), eid("b", 0)).unwrap();
        assert_eq!(
            b.add_successor(eid("a", 0), eid("b", 0)),
            Err(TaskGraphError::DuplicateSuccessor {
                from: eid("a", 0),
                to: eid("b", 0)
            })
        );
    }

    #[test]
    fn detached_task_is_rejected() {
        let mut b = TaskGraphBuilder::new("p1", 0).unwrap();
        b.add_node(node("root", 0)).unwrap();
        b.add_node(node("lost", 0)).unwrap();
        b.set_root(eid("root", 0)).unwrap();
        assert_eq!(
            b.build(),
            Err(TaskGraphError::UnreachableTasks(vec![eid("lost", 0)]))
        );
    }
}
