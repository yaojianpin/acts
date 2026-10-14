use crate::scheduler::core::model::model_graph::NodeId;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExecutionId {
    pub(crate) task_id: String,
    pub(crate) generation: u64,
}

impl ExecutionId {
    pub(crate) fn new(
        task_id: impl Into<String>,
        generation: u64,
    ) -> Result<Self, ExecutionGraphError> {
        let task_id = task_id.into();
        if task_id.trim().is_empty() {
            return Err(ExecutionGraphError::EmptyTaskId);
        }
        Ok(Self {
            task_id,
            generation,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionState {
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

impl ExecutionState {
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
pub(crate) struct ExecutionNode {
    pub(crate) id: ExecutionId,
    pub(crate) model_node: NodeId,
    pub(crate) state: ExecutionState,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionGraph {
    pub(crate) process_id: String,
    pub(crate) revision: u64,
    nodes: HashMap<ExecutionId, ExecutionNode>,
    children: HashMap<ExecutionId, Vec<ExecutionId>>,
    predecessors: HashMap<ExecutionId, Vec<ExecutionId>>,
    successors: HashMap<ExecutionId, Vec<ExecutionId>>,
}

impl ExecutionGraph {
    pub(crate) fn node(&self, id: &ExecutionId) -> Option<&ExecutionNode> {
        self.nodes.get(id)
    }

    pub(crate) fn children(&self, id: &ExecutionId) -> &[ExecutionId] {
        self.children.get(id).map(Vec::as_slice).unwrap_or_default()
    }
    pub(crate) fn predecessors(&self, id: &ExecutionId) -> &[ExecutionId] {
        self.predecessors
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    pub(crate) fn successors(&self, id: &ExecutionId) -> &[ExecutionId] {
        self.successors
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

pub(crate) struct ExecutionGraphBuilder {
    process_id: String,
    revision: u64,
    root: Option<ExecutionId>,
    nodes: HashMap<ExecutionId, ExecutionNode>,
    parent: HashMap<ExecutionId, ExecutionId>,
    children: HashMap<ExecutionId, Vec<ExecutionId>>,
    predecessors: HashMap<ExecutionId, Vec<ExecutionId>>,
    successors: HashMap<ExecutionId, Vec<ExecutionId>>,
}

impl ExecutionGraphBuilder {
    pub(crate) fn new(
        process_id: impl Into<String>,
        revision: u64,
    ) -> Result<Self, ExecutionGraphError> {
        let process_id = process_id.into();
        if process_id.trim().is_empty() {
            return Err(ExecutionGraphError::EmptyProcessId);
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

    pub(crate) fn add_node(&mut self, node: ExecutionNode) -> Result<(), ExecutionGraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(ExecutionGraphError::DuplicateExecution(node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub(crate) fn set_root(&mut self, id: ExecutionId) -> Result<(), ExecutionGraphError> {
        if self.root.replace(id).is_some() {
            return Err(ExecutionGraphError::MultipleRoots);
        }
        Ok(())
    }

    pub(crate) fn add_child(
        &mut self,
        parent: ExecutionId,
        child: ExecutionId,
    ) -> Result<(), ExecutionGraphError> {
        if self.parent.contains_key(&child) {
            return Err(ExecutionGraphError::MultipleParents(child));
        }
        self.parent.insert(child.clone(), parent.clone());
        self.children.entry(parent).or_default().push(child);
        Ok(())
    }

    pub(crate) fn add_successor(
        &mut self,
        from: ExecutionId,
        to: ExecutionId,
    ) -> Result<(), ExecutionGraphError> {
        if self
            .successors
            .get(&from)
            .is_some_and(|items| items.contains(&to))
        {
            return Err(ExecutionGraphError::DuplicateSuccessor { from, to });
        }
        self.predecessors
            .entry(to.clone())
            .or_default()
            .push(from.clone());
        self.successors.entry(from).or_default().push(to);
        Ok(())
    }

    pub(crate) fn build(self) -> Result<ExecutionGraph, ExecutionGraphError> {
        let root = self.root.clone().ok_or(ExecutionGraphError::MissingRoot)?;
        self.require_node(&root)?;
        if self.parent.contains_key(&root) {
            return Err(ExecutionGraphError::RootHasParent);
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
            return Err(ExecutionGraphError::UnreachableExecutions(ids));
        }

        Ok(ExecutionGraph {
            process_id: self.process_id,
            revision: self.revision,
            nodes: self.nodes,
            children: self.children,
            predecessors: self.predecessors,
            successors: self.successors,
        })
    }

    fn require_node(&self, id: &ExecutionId) -> Result<(), ExecutionGraphError> {
        if self.nodes.contains_key(id) {
            Ok(())
        } else {
            Err(ExecutionGraphError::UnknownExecution(id.clone()))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionGraphError {
    EmptyProcessId,
    EmptyTaskId,
    MissingRoot,
    MultipleRoots,
    RootHasParent,
    DuplicateExecution(ExecutionId),
    UnknownExecution(ExecutionId),
    MultipleParents(ExecutionId),
    DuplicateSuccessor { from: ExecutionId, to: ExecutionId },
    UnreachableExecutions(Vec<ExecutionId>),
}
impl fmt::Display for ExecutionGraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "execution graph error: {self:?}")
    }
}
impl std::error::Error for ExecutionGraphError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn eid(id: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(id, generation).unwrap()
    }
    fn node(id: &str, generation: u64) -> ExecutionNode {
        ExecutionNode {
            id: eid(id, generation),
            model_node: NodeId::parse(id).unwrap(),
            state: ExecutionState::Created,
            revision: 0,
        }
    }

    #[test]
    fn generations_are_distinct_execution_identity() {
        let mut b = ExecutionGraphBuilder::new("p1", 4).unwrap();
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
        let mut b = ExecutionGraphBuilder::new("p1", 0).unwrap();
        for n in [node("root", 0), node("a", 0), node("b", 0)] {
            b.add_node(n).unwrap();
        }
        b.add_successor(eid("root", 0), eid("b", 0)).unwrap();
        b.add_successor(eid("a", 0), eid("b", 0)).unwrap();
        assert_eq!(
            b.add_successor(eid("a", 0), eid("b", 0)),
            Err(ExecutionGraphError::DuplicateSuccessor {
                from: eid("a", 0),
                to: eid("b", 0)
            })
        );
    }

    #[test]
    fn detached_execution_is_rejected() {
        let mut b = ExecutionGraphBuilder::new("p1", 0).unwrap();
        b.add_node(node("root", 0)).unwrap();
        b.add_node(node("lost", 0)).unwrap();
        b.set_root(eid("root", 0)).unwrap();
        assert_eq!(
            b.build(),
            Err(ExecutionGraphError::UnreachableExecutions(vec![eid(
                "lost", 0
            )]))
        );
    }
}
