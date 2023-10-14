use serde::{Deserialize, Serialize};

use crate::{Act, Branch, Job, ModelBase, Step, Vars, Workflow};
use std::sync::{Arc, RwLock, Weak};

#[derive(Debug, Clone)]
pub enum NodeData {
    Workflow(Workflow),
    Job(Job),
    Branch(Branch),
    Step(Step),
    Act(Act),
}

#[derive(PartialEq, Default, Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind {
    #[default]
    Workflow,
    Job,
    Branch,
    Step,
    Act,
}

#[derive(Clone)]
pub struct Node {
    pub data: NodeData,
    pub level: usize,
    pub parent: Arc<RwLock<Weak<Node>>>,
    pub children: Arc<RwLock<Vec<Arc<Node>>>>,
    pub prev: Arc<RwLock<Weak<Node>>>,
    pub next: Arc<RwLock<Weak<Node>>>,

    /// used for recording visit count
    pub(in crate::sch) visit_count: Arc<RwLock<usize>>,
}

impl NodeData {
    pub fn id(&self) -> String {
        match self {
            NodeData::Workflow(data) => data.id.clone(),
            NodeData::Job(data) => data.id.clone(),
            NodeData::Branch(data) => data.id.clone(),
            NodeData::Step(data) => data.id.clone(),
            NodeData::Act(data) => data.id().to_string(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            NodeData::Workflow(data) => data.name.clone(),
            NodeData::Job(data) => data.name.clone(),
            NodeData::Branch(data) => data.name.clone(),
            NodeData::Step(data) => data.name.clone(),
            NodeData::Act(data) => data.name.clone(),
        }
    }

    pub fn inputs(&self) -> Vars {
        match self {
            NodeData::Workflow(data) => data.env.clone(),
            NodeData::Job(data) => data.inputs.clone(),
            NodeData::Branch(data) => data.inputs.clone(),
            NodeData::Step(data) => data.inputs.clone(),
            NodeData::Act(data) => data.inputs.clone(),
        }
    }

    pub fn outputs(&self) -> Vars {
        match self {
            NodeData::Workflow(data) => data.outputs.clone(),
            NodeData::Job(data) => data.outputs.clone(),
            NodeData::Branch(data) => data.outputs.clone(),
            NodeData::Step(data) => data.outputs.clone(),
            NodeData::Act(data) => data.outputs.clone(),
        }
    }
}

impl Node {
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

    pub fn data(&self) -> NodeData {
        self.data.clone()
    }

    pub fn id(&self) -> String {
        self.data.id()
    }

    pub fn name(&self) -> String {
        self.data.name()
    }

    pub fn inputs(&self) -> Vars {
        self.data.inputs()
    }

    pub fn outputs(&self) -> Vars {
        self.data.outputs()
    }

    pub fn kind(&self) -> NodeKind {
        match self.data() {
            NodeData::Workflow(_) => NodeKind::Workflow,
            NodeData::Job(_) => NodeKind::Job,
            NodeData::Branch(_) => NodeKind::Branch,
            NodeData::Step(_) => NodeKind::Step,
            NodeData::Act(_) => NodeKind::Act,
        }
    }

    pub fn tag(&self) -> String {
        match self.data() {
            NodeData::Workflow(data) => data.tag,
            NodeData::Job(data) => data.tag,
            NodeData::Branch(data) => data.tag,
            NodeData::Step(data) => data.tag,
            NodeData::Act(data) => data.tag,
        }
    }

    pub(in crate::sch) fn visit_count(&self) -> usize {
        let visit_count = self.visit_count.read().unwrap();
        *visit_count
    }

    pub(in crate::sch) fn visit(&self) {
        let mut visit_count = self.visit_count.write().unwrap();
        *visit_count += 1;
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("data", &self.data)
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
            NodeKind::Job => "job",
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
            "job" => NodeKind::Job,
            "branch" => NodeKind::Branch,
            "step" => NodeKind::Step,
            "act" => NodeKind::Act,
            _ => panic!("not found NodeKind: {}", str),
        };

        s
    }
}
