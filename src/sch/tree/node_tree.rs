use super::{
    build,
    node::{Node, NodeData},
};
use crate::{ActError, ShareLock, Workflow};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, RwLock, Weak},
};

#[derive(Clone)]
pub struct NodeTree {
    pub(crate) root: Option<Arc<Node>>,
    pub(crate) node_map: ShareLock<HashMap<String, Arc<Node>>>,
    pub(crate) error: Option<ActError>,
    pub(crate) model: Box<Workflow>,
}

impl NodeTree {
    pub fn new() -> Self {
        NodeTree {
            root: None,
            node_map: Arc::new(RwLock::new(HashMap::new())),
            error: None,
            model: Box::new(Workflow::default()),
        }
    }

    pub fn root(&self) -> Option<Arc<Node>> {
        self.root.clone()
    }

    pub fn build(workflow: &mut Workflow) -> NodeTree {
        let mut tree = NodeTree::new();
        build::build_workflow(workflow, &mut tree);

        tree
    }

    pub fn load(&mut self, model: &Workflow) {
        let mut model = model.clone();
        build::build_workflow(&mut model, self);
    }

    pub fn make(&self, data: NodeData, level: usize) -> Arc<Node> {
        let node = Arc::new(Node {
            data,
            level,
            parent: Arc::new(RwLock::new(Weak::new())),
            children: Arc::new(RwLock::new(Vec::new())),
            prev: Arc::new(RwLock::new(Weak::new())),
            next: Arc::new(RwLock::new(Weak::new())),
            visit_count: Arc::new(RwLock::new(0)),
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

    pub fn node(&self, key: &str) -> Option<Arc<Node>> {
        let map = self.node_map.read().unwrap();
        match map.get(key) {
            Some(node) => Some(node.clone()),
            None => None,
        }
    }

    #[allow(unused)]
    pub fn print(&self) {
        if let Some(node) = self.root.clone() {
            self.visit(&node, |node| {
                let mut level = node.level;
                while level > 0 {
                    print!("  ");
                    level -= 1;
                }

                let next = match node.next().upgrade() {
                    Some(n) => n.id(),
                    None => "nil".to_string(),
                };
                println!(
                    "{} id:{} name={}  next={}",
                    node.kind(),
                    node.id(),
                    node.name(),
                    next
                );
            });
        }
    }

    #[allow(unused)]
    pub fn tree_output(&self) -> String {
        let s = &RefCell::new(String::new());
        if let Some(node) = self.root.clone() {
            self.visit(&node, move |n| {
                let mut level = n.level;
                while level > 0 {
                    s.borrow_mut().push_str(&format!("{}", "  "));
                    level -= 1;
                }

                let next = match n.next().upgrade() {
                    Some(n) => n.id(),
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

    pub fn set_error(&mut self, err: ActError) {
        self.error = Some(err);
    }

    #[allow(unused)]
    pub fn visit<F: Fn(&Node) + Clone>(&self, node: &Arc<Node>, f: F) {
        f(node);
        node.visit();

        let children = node.children();
        if children.len() > 0 {
            for child in children {
                self.visit(&child, f.clone());
            }
        }

        if let Some(next) = node.next().upgrade() {
            if next.visit_count() == 0 {
                self.visit(&next, f.clone());
            }
        }
    }
}
