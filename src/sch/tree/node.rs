use serde::{Deserialize, Serialize};

use crate::{Act, Branch, ModelBase, Step, Vars, Workflow};
use std::sync::{Arc, RwLock, Weak};

#[derive(Debug, Clone)]
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

#[derive(Clone)]
pub struct Node {
    pub id: String,
    pub content: NodeContent,
    pub level: usize,
    pub parent: Arc<RwLock<Weak<Node>>>,
    pub children: Arc<RwLock<Vec<Arc<Node>>>>,
    pub prev: Arc<RwLock<Weak<Node>>>,
    pub next: Arc<RwLock<Weak<Node>>>,
}

impl NodeContent {
    pub fn id(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.id.clone(),
            NodeContent::Branch(data) => data.id.clone(),
            NodeContent::Step(data) => data.id.clone(),
            NodeContent::Act(data) => data.id().to_string(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.name.clone(),
            NodeContent::Branch(data) => data.name.clone(),
            NodeContent::Step(data) => data.name.clone(),
            NodeContent::Act(data) => data.name().to_string(),
        }
    }

    pub fn inputs(&self) -> Vars {
        match self {
            NodeContent::Workflow(c) => c.inputs.clone(),
            NodeContent::Branch(c) => c.inputs.clone(),
            NodeContent::Step(c) => c.inputs.clone(),
            NodeContent::Act(c) => c.inputs(),
        }
    }

    pub fn outputs(&self) -> Vars {
        match self {
            NodeContent::Workflow(node) => node.outputs.clone(),
            NodeContent::Branch(node) => node.outputs.clone(),
            NodeContent::Step(node) => node.outputs.clone(),
            NodeContent::Act(node) => node.outputs(),
        }
    }

    pub fn rets(&self) -> Vars {
        match self {
            NodeContent::Act(node) => node.rets(),
            _ => Vars::new(),
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

    pub fn set_parent(&self, parent: &Arc<Node>) {
        *self.parent.write().unwrap() = Arc::downgrade(&parent);
        parent
            .children
            .write()
            .unwrap()
            .push(Arc::new(self.clone()));
    }

    pub fn set_next(self: &Arc<Node>, node: &Arc<Node>, is_prev: bool) {
        *self.next.write().unwrap() = Arc::downgrade(node);
        if is_prev {
            *node.prev.write().unwrap() = Arc::downgrade(self);
        }
    }

    pub fn children(&self) -> Vec<Arc<Node>> {
        let node = self.children.read().unwrap();
        node.clone()
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

    pub fn r#type(&self) -> String {
        if let NodeContent::Act(act) = &self.content {
            return act.kind().to_string();
        }

        self.kind().to_string()
    }

    pub fn tag(&self) -> &str {
        match &self.content {
            NodeContent::Workflow(data) => &data.tag,
            NodeContent::Branch(data) => &data.tag,
            NodeContent::Step(data) => &data.tag,
            NodeContent::Act(data) => data.tag(),
        }
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
        let s = self.clone().into();
        f.write_str(s)
    }
}

impl<'a> Into<&'a str> for NodeKind {
    fn into(self) -> &'a str {
        match self {
            NodeKind::Workflow => "workflow",
            NodeKind::Branch => "branch",
            NodeKind::Step => "step",
            NodeKind::Act => "act",
        }
    }
}

impl Into<String> for NodeKind {
    fn into(self) -> String {
        let s: &str = self.into();
        s.to_string()
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
        let s = match str {
            "workflow" => NodeKind::Workflow,
            "branch" => NodeKind::Branch,
            "step" => NodeKind::Step,
            "act" => NodeKind::Act,
            _ => panic!("not found NodeKind: {}", str),
        };

        s
    }
}
