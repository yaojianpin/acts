use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId(String);

impl NodeId {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, GraphError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(GraphError::EmptyNodeId);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one node of the *policy* graph is, in the model's own vocabulary.
///
/// The graph the planners walk holds the workflow root and the model's
/// top-level steps — the fan-outs (a step's `branches`, their acts) are
/// plan-level, not graph nodes. The four variants are the model's four node
/// kinds, kept whole so this vocabulary matches [`crate::NodeKind`] (the
/// `view` one the read model and the messages speak); a graph that only
/// materializes steps still cannot say "step" if the word is missing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeKind {
    Workflow,
    Step,
    /// A model node kind this graph does not materialize as a node: a step's
    /// `branches` are plan-level fan-outs, not graph nodes. Kept so the
    /// vocabulary stays the model's own (see the type's docs).
    #[allow(dead_code)]
    // no constructor in a non-test build」
    Branch,
    /// The `uses` node of a step — likewise plan-level today.
    #[allow(dead_code)] // see `Branch`
    Act,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NodeDefinition {
    pub(crate) id: NodeId,
    pub(crate) kind: NodeKind,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChildRole {
    Normal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelGraph {
    declaration_next: HashMap<NodeId, NodeId>,
    explicit_next: HashMap<NodeId, NodeId>,
}

impl ModelGraph {
    pub(crate) fn successor(&self, id: &NodeId) -> Option<&NodeId> {
        self.explicit_next
            .get(id)
            .or_else(|| self.declaration_next.get(id))
    }
}

#[derive(Default)]
pub(crate) struct ModelGraphBuilder {
    root: Option<NodeId>,
    nodes: HashMap<NodeId, NodeDefinition>,
    parent: HashMap<NodeId, (NodeId, ChildRole)>,
    children: HashMap<NodeId, Vec<NodeId>>,
    explicit_next: HashMap<NodeId, NodeId>,
}

impl ModelGraphBuilder {
    pub(crate) fn add_node(&mut self, node: NodeDefinition) -> Result<(), GraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(GraphError::DuplicateNode(node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub(crate) fn set_root(&mut self, id: NodeId) -> Result<(), GraphError> {
        if self.root.replace(id).is_some() {
            return Err(GraphError::MultipleRoots);
        }
        Ok(())
    }

    pub(crate) fn add_child(
        &mut self,
        parent: NodeId,
        child: NodeId,
        role: ChildRole,
    ) -> Result<(), GraphError> {
        if self.parent.contains_key(&child) {
            return Err(GraphError::MultipleParents(child));
        }
        self.parent.insert(child.clone(), (parent.clone(), role));
        self.children.entry(parent).or_default().push(child);
        Ok(())
    }

    pub(crate) fn set_explicit_next(&mut self, from: NodeId, to: NodeId) -> Result<(), GraphError> {
        if self.explicit_next.contains_key(&from) {
            return Err(GraphError::MultipleExplicitNext(from));
        }
        self.explicit_next.insert(from, to);
        Ok(())
    }

    pub(crate) fn build(self) -> Result<ModelGraph, GraphError> {
        let root = self.root.ok_or(GraphError::MissingRoot)?;
        if !self.nodes.contains_key(&root) {
            return Err(GraphError::UnknownNode(root));
        }

        for (child, (parent, _)) in &self.parent {
            if !self.nodes.contains_key(child) {
                return Err(GraphError::UnknownNode(child.clone()));
            }
            if !self.nodes.contains_key(parent) {
                return Err(GraphError::UnknownNode(parent.clone()));
            }
            if child == &root {
                return Err(GraphError::RootHasParent);
            }
        }
        for (from, to) in &self.explicit_next {
            if !self.nodes.contains_key(from) {
                return Err(GraphError::UnknownNode(from.clone()));
            }
            if !self.nodes.contains_key(to) {
                return Err(GraphError::UnknownNode(to.clone()));
            }
        }

        let mut declaration_next = HashMap::new();
        for children in self.children.values() {
            for pair in children.windows(2) {
                declaration_next.insert(pair[0].clone(), pair[1].clone());
            }
        }

        let mut visited = HashSet::new();
        let mut stack = vec![root.clone()];
        while let Some(id) = stack.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if let Some(children) = self.children.get(&id) {
                stack.extend(children.iter().cloned());
            }
        }
        if visited.len() != self.nodes.len() {
            let mut unreachable = self
                .nodes
                .keys()
                .filter(|id| !visited.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            unreachable.sort();
            return Err(GraphError::UnreachableNodes(unreachable));
        }

        Ok(ModelGraph {
            declaration_next,
            explicit_next: self.explicit_next,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GraphError {
    EmptyNodeId,
    MissingRoot,
    MultipleRoots,
    RootHasParent,
    DuplicateNode(NodeId),
    UnknownNode(NodeId),
    MultipleParents(NodeId),
    MultipleExplicitNext(NodeId),
    UnreachableNodes(Vec<NodeId>),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "model graph error: {self:?}")
    }
}
impl std::error::Error for GraphError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(v: &str) -> NodeId {
        NodeId::parse(v).unwrap()
    }
    fn node(v: &str, kind: NodeKind) -> NodeDefinition {
        NodeDefinition {
            id: id(v),
            kind,
            name: v.into(),
        }
    }

    #[test]
    fn explicit_next_does_not_destroy_declaration_order() {
        let mut b = ModelGraphBuilder::default();
        for n in [
            node("root", NodeKind::Workflow),
            node("a", NodeKind::Act),
            node("b", NodeKind::Act),
            node("c", NodeKind::Act),
        ] {
            b.add_node(n).unwrap();
        }
        b.set_root(id("root")).unwrap();
        for child in ["a", "b", "c"] {
            b.add_child(id("root"), id(child), ChildRole::Normal)
                .unwrap();
        }
        b.set_explicit_next(id("a"), id("c")).unwrap();
        let g = b.build().unwrap();
        assert_eq!(g.successor(&id("a")), Some(&id("c")));
        assert_eq!(g.successor(&id("b")), Some(&id("c")));
    }

    #[test]
    fn duplicate_identity_is_rejected_not_renamed() {
        let mut b = ModelGraphBuilder::default();
        b.add_node(node("a", NodeKind::Act)).unwrap();
        assert_eq!(
            b.add_node(node("a", NodeKind::Act)),
            Err(GraphError::DuplicateNode(id("a")))
        );
    }

    #[test]
    fn shared_child_is_rejected() {
        let mut b = ModelGraphBuilder::default();
        for n in [
            node("root", NodeKind::Workflow),
            node("a", NodeKind::Act),
            node("b", NodeKind::Act),
        ] {
            b.add_node(n).unwrap();
        }
        b.add_child(id("root"), id("b"), ChildRole::Normal).unwrap();
        assert_eq!(
            b.add_child(id("a"), id("b"), ChildRole::Normal),
            Err(GraphError::MultipleParents(id("b")))
        );
    }

    #[test]
    fn disconnected_nodes_are_rejected() {
        let mut b = ModelGraphBuilder::default();
        b.add_node(node("root", NodeKind::Workflow)).unwrap();
        b.add_node(node("lost", NodeKind::Act)).unwrap();
        b.set_root(id("root")).unwrap();
        assert_eq!(
            b.build(),
            Err(GraphError::UnreachableNodes(vec![id("lost")]))
        );
    }
}
