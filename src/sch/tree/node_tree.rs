use super::{
    node::{Node, NodeData},
    utils,
};
use crate::{debug, ActError, ShareLock, Workflow};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock, Weak},
};

#[derive(Clone)]
pub struct NodeTree {
    pub(crate) root: Option<Arc<Node>>,
    pub(crate) node_map: ShareLock<HashMap<String, Arc<Node>>>,
    pub(crate) error: Option<ActError>,
}

impl NodeTree {
    pub fn new() -> Self {
        NodeTree {
            root: None,
            node_map: Arc::new(RwLock::new(HashMap::new())),
            error: None,
        }
    }

    pub fn build(workflow: &mut Workflow) -> Arc<NodeTree> {
        let mut tree = NodeTree::new();
        utils::process_workflow(workflow, &mut tree);

        Arc::new(tree)
    }

    pub fn make(&self, root: &str, data: NodeData, level: usize) -> Arc<Node> {
        let node = Arc::new(Node {
            root: root.to_string(),
            data: data,
            level,
            parent: Arc::new(RwLock::new(Weak::new())),
            children: Arc::new(RwLock::new(Vec::new())),
            next: Arc::new(RwLock::new(Weak::new())),
        });

        self.node_map
            .write()
            .unwrap()
            .insert(node.id(), node.clone());

        node
    }

    pub fn set_root(&mut self, node: &Arc<Node>) {
        self.root = Some(node.clone());
    }

    pub fn push_act(&self, act: &NodeData, step_id: &str) -> Arc<Node> {
        debug!("push_act step_id={} act={:?}", step_id, act);
        let parent = self.node(step_id).unwrap();
        let node = self.make(&parent.root, act.clone(), parent.level + 1);
        node.set_parent(&parent);
        self.node_map
            .write()
            .unwrap()
            .insert(act.id(), node.clone());

        node
    }

    pub fn node(&self, key: &str) -> Option<Arc<Node>> {
        let map = self.node_map.read().unwrap();
        match map.get(key) {
            Some(node) => Some(node.clone()),
            None => None,
        }
    }

    #[allow(unused)]
    pub fn print(&self) {
        println!("print:");
        if let Some(root) = self.root.clone() {
            self.visit_(&root, |node| {
                let mut level = node.level;
                while level > 0 {
                    println!("  ");
                    level -= 1;
                }
                println!("{:?}\n", node.data());
            });
        }
    }

    #[allow(unused)]
    pub fn walk<F: Fn(&Node) + Clone>(&self, f: F) {
        if let Some(node) = self.root.clone() {
            self.visit_(&node, f);
        }
    }

    pub fn set_error(&mut self, err: ActError) {
        self.error = Some(err);
    }

    fn visit_<F: Fn(&Node) + Clone>(&self, node: &Arc<Node>, f: F) {
        f(node);

        let children = node.children.read().unwrap();
        if children.len() > 0 {
            let next = &children[0];
            self.visit_(next, f.clone());
        }

        let next = node.next.read().unwrap();
        if let Some(next) = next.upgrade() {
            // just visit the same level, or it will be recursive
            if next.level == node.level {
                self.visit_(&next, f.clone());
            }
        }
    }
}
