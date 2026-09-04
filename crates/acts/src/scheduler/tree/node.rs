use crate::{Act, ActError, Branch, Result, Step, Variant, Vars, Workflow};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Weak};

use super::{node_tree, visit::VisitRoot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeContent {
    Workflow(Workflow),
    Branch(Branch),
    Step(Step),
    Act(Act),
}

#[derive(PartialEq, Default, Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind {
    #[default]
    Workflow,
    Branch,
    Step,
    Act,
}

#[derive(PartialEq, Default, Copy, Debug, Clone, Serialize, Deserialize)]
pub enum NodeOutputKind {
    #[default]
    Normal,
    Catch,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct NodeOutput {
    pub typ: NodeOutputKind,
    pub node: Arc<Node>,
}

#[derive(Clone)]
pub struct Node {
    pub id: String,
    pub content: NodeContent,
    pub level: usize,
    pub parent: Arc<RwLock<Weak<Node>>>,
    pub children: Arc<RwLock<Vec<NodeOutput>>>,

    /// previous node in the declaration chain
    pub prev: Arc<RwLock<Weak<Node>>>,

    /// where the flow goes after this node completes — the step's explicit
    /// `next` target when one is declared, otherwise the node that follows it
    /// in the declaration chain
    pub next: Arc<RwLock<Weak<Node>>>,

    /// next node in the declaration chain — the default flow a step skipped
    /// by its `if`/`while` condition falls through to. It is never an explicit
    /// jump target, so a following step can never clobber a declared `next`.
    pub chain: Arc<RwLock<Weak<Node>>>,

    // nodes created dynamically
    pub nodes: Arc<RwLock<Vec<Arc<Node>>>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    pub id: String,
    pub content: NodeContent,
    pub level: usize,
    /// parent node id, persisted for dynamic nodes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// previous node id in the dynamic chain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_id: Option<String>,
    /// next node id in the dynamic chain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_id: Option<String>,
}

impl NodeContent {
    pub fn id(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.id.clone(),
            NodeContent::Branch(data) => data.id.clone(),
            NodeContent::Step(data) => data.id.clone(),
            NodeContent::Act(data) => data.id.to_string(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.name.clone(),
            NodeContent::Branch(data) => data.name.clone(),
            NodeContent::Step(data) => data.name.clone(),
            NodeContent::Act(data) => data.name.to_string(),
        }
    }

    pub fn vars(&self) -> Vars {
        match self {
            NodeContent::Workflow(data) => data.vars(),
            NodeContent::Branch(data) => data.vars(),
            NodeContent::Step(data) => data.vars(),
            NodeContent::Act(data) => data.vars(),
        }
    }

    pub fn options(&self) -> Vars {
        match self {
            NodeContent::Workflow(node) => node.options.clone(),
            NodeContent::Branch(node) => node.options.clone(),
            NodeContent::Step(node) => node.options.clone(),
            NodeContent::Act(node) => node.options.clone(),
        }
    }

    pub fn exposes(&self) -> &Vec<Variant> {
        match self {
            NodeContent::Workflow(node) => &node.exposes,
            NodeContent::Branch(node) => &node.exposes,
            NodeContent::Step(node) => &node.exposes,
            NodeContent::Act(node) => &node.exposes,
        }
    }

    pub fn params(&self) -> serde_json::Value {
        match self {
            NodeContent::Step(node) => node.params.clone(),
            NodeContent::Act(node) => node.params.clone(),
            _ => serde_json::Value::Null,
        }
    }

    pub fn r#if(&self) -> Option<String> {
        match self {
            NodeContent::Step(node) => node.r#if.clone(),
            NodeContent::Branch(node) => node.r#if.clone(),
            NodeContent::Act(node) => node.r#if.clone(),
            _ => None,
        }
    }

    pub fn set_if(&mut self, v: Option<String>) {
        match self {
            NodeContent::Step(node) => node.r#if = v,
            NodeContent::Branch(node) => node.r#if = v,
            NodeContent::Act(node) => node.r#if = v,
            _ => {}
        }
    }
}

impl Node {
    pub fn new(id: &str, data: NodeContent, level: usize) -> Self {
        Self {
            id: id.to_string(),
            content: data,
            level,
            parent: Arc::new(RwLock::new(Weak::new())),
            children: Arc::new(RwLock::new(Vec::new())),
            prev: Arc::new(RwLock::new(Weak::new())),
            next: Arc::new(RwLock::new(Weak::new())),
            chain: Arc::new(RwLock::new(Weak::new())),
            nodes: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn parent(&self) -> Option<Arc<Node>> {
        let node = self.parent.read();
        if let Some(parent) = node.upgrade() {
            return Some(parent);
        }

        if let Some(prev) = self.prev().upgrade() {
            return prev.parent();
        }

        None
    }

    pub fn push_child(&self, typ: NodeOutputKind, child: &Arc<Node>) {
        let mut children = self.children.write();
        children.push(NodeOutput {
            typ,
            node: child.clone(),
        });
    }

    pub fn set_parent(&self, parent: &Arc<Node>) {
        self.set_parent_in(NodeOutputKind::Normal, parent);
    }

    /// set parent in the node tree with the given type
    pub fn set_parent_in(&self, typ: NodeOutputKind, parent: &Arc<Node>) {
        *self.parent.write() = Arc::downgrade(parent);
        parent.children.write().push(NodeOutput {
            typ,
            node: Arc::new(self.clone()),
        });
    }

    pub fn set_next(self: &Arc<Node>, node: &Arc<Node>, is_prev: bool) {
        *self.next.write() = Arc::downgrade(node);
        if is_prev {
            *node.prev.write() = Arc::downgrade(self);
        }
    }

    /// Link `node` as `self`'s declaration-order successor: always records
    /// `chain`; `next` only when `self` has no explicit `next` of its own, so a
    /// later sibling can never clobber a step's declared jump target.
    pub fn link_chain(self: &Arc<Node>, node: &Arc<Node>) {
        *self.chain.write() = Arc::downgrade(node);
        let has_explicit = matches!(&self.content, NodeContent::Step(step) if step.next.is_some());
        if !has_explicit {
            *self.next.write() = Arc::downgrade(node);
        }
        *node.prev.write() = Arc::downgrade(self);
    }

    /// rebuild the node graph links (parent/prev/next) from persisted data.
    /// children are restored as the inverse of each child's parent link.
    pub fn restore_links(self: &Arc<Self>, data: &NodeData, tree: &node_tree::NodeTree) {
        if let Some(parent_id) = &data.parent_id
            && let Some(parent) = tree.node(parent_id)
        {
            self.set_parent(&parent);
        }
        if let Some(prev_id) = &data.prev_id
            && let Some(prev) = tree.node(prev_id)
        {
            *self.prev.write() = Arc::downgrade(&prev);
        }
        if let Some(next_id) = &data.next_id
            && let Some(next) = tree.node(next_id)
        {
            *self.next.write() = Arc::downgrade(&next);
        }
    }

    pub fn children(&self) -> Vec<Arc<Node>> {
        self.children_in(NodeOutputKind::Normal)
    }

    pub fn children_in(&self, typ: NodeOutputKind) -> Vec<Arc<Node>> {
        let node = self.children.read();
        node.iter()
            .filter(|n| n.typ == typ)
            .map(|n| n.node.clone())
            .collect::<Vec<_>>()
    }

    pub fn next(&self) -> Weak<Node> {
        let next = self.next.read();
        next.clone()
    }

    pub fn chain(&self) -> Weak<Node> {
        let chain = self.chain.read();
        chain.clone()
    }

    pub fn prev(&self) -> Weak<Node> {
        let prev = self.prev.read();
        prev.clone()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn uses(&self) -> Option<String> {
        match &self.content {
            NodeContent::Step(step) => step.uses.clone(),
            NodeContent::Act(act) => Some(act.uses.to_string()),
            _ => None,
        }
    }

    pub fn params(&self) -> Value {
        match &self.content {
            NodeContent::Step(step) => step.params.clone(),
            NodeContent::Act(act) => act.params.clone(),
            _ => Value::Null,
        }
    }

    pub fn name(&self) -> String {
        self.content.name()
    }

    pub fn kind(&self) -> NodeKind {
        match &self.content {
            NodeContent::Workflow(_) => NodeKind::Workflow,
            NodeContent::Branch(_) => NodeKind::Branch,
            NodeContent::Step(_) => NodeKind::Step,
            NodeContent::Act(_) => NodeKind::Act,
        }
    }

    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> Result<String> {
        let data = NodeData {
            id: self.id.clone(),
            content: self.content.clone(),
            level: self.level,
            parent_id: self.parent.read().upgrade().map(|n| n.id.clone()),
            prev_id: self.prev().upgrade().map(|n| n.id.clone()),
            next_id: self.next().upgrade().map(|n| n.id.clone()),
        };
        serde_json::to_string(&data).map_err(|err| ActError::Store(err.to_string()))
    }

    pub fn from_str(s: &str, tree: &node_tree::NodeTree) -> Result<Arc<Self>> {
        let data: NodeData =
            serde_json::from_str(s).map_err(|err| ActError::Store(err.to_string()))?;
        let ret = Arc::new(Self::new(&data.id, data.content, data.level));
        if let Some(node) = tree.node(&ret.id) {
            return Ok(node);
        }

        Ok(ret)
    }

    #[allow(unused)]
    pub fn print(self: &Arc<Self>) {
        VisitRoot::walk(self, &move |n| {
            // print single line
            if n.level > 0 {
                for index in 1..n.level {
                    if n.path.contains_key(&index) {
                        if n.path[&index] {
                            print!("│   ");
                        } else {
                            print!("    ");
                        }
                    }
                }
                if n.is_last {
                    print!("└── ");
                } else {
                    print!("├── ");
                }
            }
            let next = match n.next().upgrade() {
                Some(n) => n.id().to_string(),
                None => "nil".to_string(),
            };

            if n.kind() == NodeKind::Act {
                println!(
                    "{}:{} uses={} name={}  next={}",
                    n.kind(),
                    n.id(),
                    n.uses().unwrap_or("nil".to_string()),
                    n.name(),
                    next,
                );
            } else {
                println!("{}:{} name={}  next={}", n.kind(), n.id(), n.name(), next);
            }
        });
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("data", &self.content)
            .field("level", &self.level)
            .field("parent", &self.parent)
            .field("children", &self.children)
            .field("next", &self.next)
            .finish()
    }
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NodeKind::Workflow => "workflow",
            NodeKind::Branch => "branch",
            NodeKind::Step => "step",
            NodeKind::Act => "act",
        };
        f.write_str(s)
    }
}

impl From<NodeKind> for String {
    fn from(value: NodeKind) -> Self {
        value.to_string()
    }
}

impl From<String> for NodeKind {
    fn from(kind: String) -> Self {
        let s: &str = &kind;
        s.into()
    }
}

impl From<&str> for NodeKind {
    fn from(str: &str) -> Self {
        match str {
            "workflow" => NodeKind::Workflow,
            "branch" => NodeKind::Branch,
            "step" => NodeKind::Step,
            "act" => NodeKind::Act,
            _ => panic!("not found NodeKind: {str}"),
        }
    }
}
