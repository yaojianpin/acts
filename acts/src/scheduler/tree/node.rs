use crate::{Act, Branch, Step, Vars, Workflow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock, Weak};

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

#[derive(PartialEq, Default, Debug, Clone, Serialize, Deserialize)]
pub enum NodeOutputKind {
    #[default]
    Normal,
    Catch,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct NodeOutput {
    pub typ: NodeOutputKind,
    pub on: Option<String>,
    pub node: Arc<Node>,
}

#[derive(Clone)]
pub struct Node {
    pub id: String,
    pub content: NodeContent,
    pub level: usize,
    pub parent: Arc<RwLock<Weak<Node>>>,
    pub children: Arc<RwLock<Vec<NodeOutput>>>,
    pub prev: Arc<RwLock<Weak<Node>>>,
    pub next: Arc<RwLock<Weak<Node>>>,

    // nodes created dynamically
    pub nodes: Arc<RwLock<Vec<Arc<Node>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    pub id: String,
    pub content: NodeContent,
    pub level: usize,
    // pub parent: Option<String>,
    // pub children: Vec<String>,
    // pub prev: Option<String>,
    // pub next: Option<String>,
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

    pub fn inputs(&self) -> Vars {
        match self {
            NodeContent::Workflow(c) => c.inputs.clone(),
            NodeContent::Branch(c) => c.inputs.clone(),
            NodeContent::Step(c) => c.inputs.clone(),
            NodeContent::Act(c) => c.inputs.clone(),
        }
    }

    pub fn outputs(&self) -> Vars {
        match self {
            NodeContent::Workflow(node) => node.outputs.clone(),
            NodeContent::Branch(node) => node.outputs.clone(),
            NodeContent::Step(node) => node.outputs.clone(),
            NodeContent::Act(node) => node.outputs.clone(),
        }
    }

    pub fn options(&self) -> Vars {
        match self {
            NodeContent::Act(node) => node.options.clone(),
            _ => Vars::new(),
        }
    }

    pub fn params(&self) -> serde_json::Value {
        match self {
            NodeContent::Act(node) => node.params.clone(),
            _ => serde_json::Value::Null,
        }
    }

    pub fn tag(&self) -> String {
        match self {
            NodeContent::Workflow(node) => node.tag.clone(),
            NodeContent::Branch(node) => node.tag.clone(),
            NodeContent::Step(node) => node.tag.clone(),
            NodeContent::Act(node) => node.tag.clone(),
        }
    }

    /// only the act has the key
    pub fn key(&self) -> String {
        match self {
            NodeContent::Act(node) => node.key.clone(),
            _ => "".to_string(),
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
            nodes: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn parent(&self) -> Option<Arc<Node>> {
        let node = self.parent.read().unwrap();
        if let Some(parent) = node.upgrade() {
            return Some(parent);
        }

        if let Some(prev) = self.prev().upgrade() {
            return prev.parent();
        }

        None
    }

    pub fn push_child(&self, typ: NodeOutputKind, on: Option<String>, child: &Arc<Node>) {
        let mut children = self.children.write().unwrap();
        children.push(NodeOutput {
            typ,
            on,
            node: child.clone(),
        });
    }

    pub fn set_parent(&self, parent: &Arc<Node>) {
        self.set_parent_in(NodeOutputKind::Normal, None, parent);
    }

    /// set parent in the node tree with the given type
    pub fn set_parent_in(&self, typ: NodeOutputKind, on: Option<String>, parent: &Arc<Node>) {
        *self.parent.write().unwrap() = Arc::downgrade(parent);
        parent.children.write().unwrap().push(NodeOutput {
            typ,
            on,
            node: Arc::new(self.clone()),
        });
    }

    pub fn set_next(self: &Arc<Node>, node: &Arc<Node>, is_prev: bool) {
        *self.next.write().unwrap() = Arc::downgrade(node);
        if is_prev {
            *node.prev.write().unwrap() = Arc::downgrade(self);
        }
    }

    pub fn append_node(self: &Arc<Node>, id: &str, data: NodeContent, level: usize) -> Arc<Node> {
        let node = Arc::new(Node::new(id, data.clone(), level));
        self.nodes.write().unwrap().push(node.clone());
        node
    }

    // pub fn insert_node(
    //     self: &Arc<Node>,
    //     index: usize,
    //     node: NodeContent,
    //     level: usize,
    // ) -> Arc<Node> {
    //     let id = node.id();
    //     self.nodes.write().unwrap().insert(index, node.clone());
    //     Arc::new(Node::new(&id, node, level))
    // }

    // fn set_prev(self: &Arc<Node>, node: &Arc<Node>, is_next: bool) {
    //     *self.prev.write().unwrap() = Arc::downgrade(node);
    //     if is_next {
    //         *node.next.write().unwrap() = Arc::downgrade(self);
    //     }
    // }

    pub fn children(&self) -> Vec<Arc<Node>> {
        self.children_in(NodeOutputKind::Normal, None)
    }

    pub fn children_in(&self, typ: NodeOutputKind, on: Option<String>) -> Vec<Arc<Node>> {
        let node = self.children.read().unwrap();
        node.iter()
            .filter(|n| n.typ == typ && n.on == on)
            .map(|n| n.node.clone())
            .collect::<Vec<_>>()
    }

    pub fn next(&self) -> Weak<Node> {
        let next = self.next.read().unwrap();
        next.clone()
    }

    pub fn prev(&self) -> Weak<Node> {
        let prev = self.prev.read().unwrap();
        prev.clone()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn key(&self) -> String {
        let key = self.content.key();
        if key.is_empty() {
            // if the key is empty, use the id as key
            return self.id.clone();
        }

        key
    }

    pub fn uses(&self) -> String {
        if let NodeContent::Act(act) = &self.content {
            act.uses.to_string()
        } else {
            "".to_string()
        }
    }

    pub fn name(&self) -> String {
        self.content.name()
    }

    pub fn outputs(&self) -> Vars {
        self.content.outputs()
    }

    pub fn kind(&self) -> NodeKind {
        match &self.content {
            NodeContent::Workflow(_) => NodeKind::Workflow,
            NodeContent::Branch(_) => NodeKind::Branch,
            NodeContent::Step(_) => NodeKind::Step,
            NodeContent::Act(_) => NodeKind::Act,
        }
    }

    pub fn tag(&self) -> String {
        self.content.tag()
    }

    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let data = NodeData {
            id: self.id.clone(),
            content: self.content.clone(),
            level: self.level,
        };
        serde_json::to_string(&data).unwrap()
    }

    pub fn from_str(s: &str, tree: &node_tree::NodeTree) -> Arc<Self> {
        let data: NodeData = serde_json::from_str(s).unwrap();
        let ret = Arc::new(Self::new(&data.id, data.content, data.level));
        if let Some(node) = tree.node(&ret.id) {
            return node;
        }
        // for c in &data.children {
        //     if let Some(n) = tree.node(c) {
        //         ret.push_child(&n);
        //     }
        // }
        // if let Some(parent) = &data.parent {
        //     if let Some(n) = tree.node(parent) {
        //         ret.set_parent(&n);
        //     }
        // }

        // if let Some(prev) = &data.prev {
        //     if let Some(n) = tree.node(prev) {
        //         ret.set_prev(&n, false);
        //     }
        // }

        // if let Some(next) = &data.next {
        //     if let Some(n) = tree.node(next) {
        //         ret.set_next(&n, false);
        //     }
        // }

        ret
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
                    "{}:{} key={} uses={} name={}  next={}",
                    n.kind(),
                    n.id(),
                    n.content.key(),
                    n.uses(),
                    n.name(),
                    next,
                );
            } else {
                println!(
                    "{}:{} key={} name={}  next={}",
                    n.kind(),
                    n.id(),
                    n.content.key(),
                    n.name(),
                    next,
                );
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
            _ => panic!("not found NodeKind: {}", str),
        }
    }
}
