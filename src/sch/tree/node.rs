use crate::{Branch, Job, Step, Workflow};
use std::sync::{Arc, RwLock, Weak};

#[derive(Debug, Clone)]
pub enum NodeData {
    Workflow(Workflow),
    Job(Job),
    Branch(Branch),
    Step(Step),
}

#[derive(PartialEq, Debug, Clone)]
pub enum NodeKind {
    Workflow,
    Job,
    Branch,
    Step,
}

#[derive(Clone)]
pub struct Node {
    pub root: String,
    pub data: NodeData,
    pub level: usize,
    pub parent: Arc<RwLock<Weak<Node>>>,
    pub children: Arc<RwLock<Vec<Arc<Node>>>>,
    pub next: Arc<RwLock<Weak<Node>>>,
}

impl NodeData {
    pub fn id(&self) -> String {
        match self {
            NodeData::Workflow(data) => data.id.clone(),
            NodeData::Job(data) => data.id.clone(),
            NodeData::Branch(data) => data.id.clone(),
            NodeData::Step(data) => data.id.clone(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            NodeData::Workflow(data) => data.name.clone(),
            NodeData::Job(data) => data.name.clone(),
            NodeData::Branch(data) => data.name.clone(),
            NodeData::Step(data) => data.name.clone(),
        }
    }
}

impl Node {
    pub fn parent(&self) -> Option<Arc<Node>> {
        let node = self.parent.read().unwrap();
        node.upgrade()
    }

    pub fn set_parent(&self, parent: &Arc<Node>) {
        *self.parent.write().unwrap() = Arc::downgrade(&parent);
        parent
            .children
            .write()
            .unwrap()
            .push(Arc::new(self.clone()));
    }

    pub fn set_next(&self, node: &Arc<Node>) {
        *self.next.write().unwrap() = Arc::downgrade(node);
    }

    pub fn children(&self) -> Vec<Arc<Node>> {
        let node = self.children.read().unwrap();
        node.clone()
    }

    pub fn next(&self) -> Weak<Node> {
        let next = self.next.read().unwrap();
        next.clone()
    }

    pub fn data(&self) -> NodeData {
        self.data.clone()
    }

    pub fn id(&self) -> String {
        self.data.id()
    }

    pub fn kind(&self) -> NodeKind {
        match self.data() {
            NodeData::Workflow(_) => NodeKind::Workflow,
            NodeData::Job(_) => NodeKind::Job,
            NodeData::Branch(_) => NodeKind::Branch,
            NodeData::Step(_) => NodeKind::Step,
        }
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
            _ => panic!("not found NodeKind: {}", str),
        };

        s
    }
}
