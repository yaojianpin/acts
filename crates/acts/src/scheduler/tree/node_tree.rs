use super::{
    build,
    node::{Node, NodeContent},
    visit::VisitRoot,
};
use crate::utils::shortid;
use crate::{ActError, Result, Workflow};
use parking_lot::RwLock;
use std::sync::Arc;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

#[derive(Default, Clone)]
pub struct NodeTree {
    pub(crate) root: Option<Arc<Node>>,
    pub(crate) node_map: Arc<RwLock<HashMap<String, Arc<Node>>>>,
    pub(crate) model: Box<Workflow>,
}

impl NodeTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn root(&self) -> Option<Arc<Node>> {
        self.root.clone()
    }

    pub fn build(workflow: &mut Workflow) -> Result<NodeTree> {
        let mut tree = NodeTree::new();
        build::build_workflow(workflow, &mut tree)?;

        Ok(tree)
    }

    pub fn load(&mut self, model: &Workflow) -> Result<()> {
        let mut model = model.clone();
        let mut on_ids = HashSet::new();

        for on in model.on.iter() {
            // validate trigger declarations (id/kind/config) — triggers are
            // not process nodes, so nothing is inserted into the tree
            on.valid()?;
            if !on_ids.insert(on.id.as_str()) {
                return Err(ActError::Model(format!("dup event id with '{}'", on.id)));
            }
        }

        build::build_workflow(&mut model, self)
    }

    pub fn make(&self, id: &str, data: NodeContent, level: usize) -> Result<Arc<Node>> {
        let node = Arc::new(Node::new(id, data, level));
        let mut node_map = self.node_map.write();
        if node_map.contains_key(node.id()) {
            return Err(ActError::Model(format!("dup node id with '{}'", node.id())));
        }
        node_map.insert(node.id().to_string(), node.clone());

        Ok(node)
    }

    /// create a node in the tree map, reusing the existing node if the id already exists
    pub fn get_or_make(&self, id: &str, data: NodeContent, level: usize) -> Result<Arc<Node>> {
        if let Some(node) = self.node(id) {
            return Ok(node);
        }
        self.make(id, data, level)
    }

    /// create a dynamic node under parent and register it in the tree map.
    /// node ids must be unique so persisted links resolve on restore;
    /// parallel acts may share the same model id, so uniquify on collision
    pub fn append_node(
        &self,
        parent: &Arc<Node>,
        id: &str,
        data: NodeContent,
        level: usize,
    ) -> Result<Arc<Node>> {
        let mut node_id = id.to_string();
        if self.node(&node_id).is_some() {
            node_id = format!("{}-{}", node_id, shortid());
        }
        let node = self.make(&node_id, data, level)?;
        parent.nodes.write().push(node.clone());
        Ok(node)
    }

    pub fn set_root(&mut self, node: &Arc<Node>) {
        self.root = Some(node.clone());
    }

    pub fn node(&self, key: &str) -> Option<Arc<Node>> {
        let map = self.node_map.read();
        map.get(key).cloned()
    }

    #[allow(unused)]
    pub fn print(&self) {
        if let Some(ref root) = self.root.clone() {
            root.print();
        }
    }

    #[allow(unused)]
    pub fn tree_output(&self) -> String {
        let s = &RefCell::new(String::new());
        if let Some(ref root) = self.root.clone() {
            VisitRoot::walk(root, &move |n| {
                // print single line
                if n.level > 0 {
                    for index in 1..n.level {
                        if n.path[&index] {
                            s.borrow_mut().push_str("│   ");
                        } else {
                            s.borrow_mut().push_str("    ");
                        }
                    }
                    if n.is_last {
                        s.borrow_mut().push_str("└── ");
                    } else {
                        s.borrow_mut().push_str("├── ");
                    }
                }

                let next = match n.next().upgrade() {
                    Some(n) => n.id().to_string(),
                    None => "nil".to_string(),
                };

                s.borrow_mut().push_str(&format!(
                    "{} id:{} name={}  next={}\n",
                    n.kind(),
                    n.id(),
                    n.name(),
                    next
                ));
            });
        }
        s.clone().into_inner()
    }
}
