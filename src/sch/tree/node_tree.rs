use super::{
    build,
    node::{Node, NodeContent},
    visit::VisitRoot,
};
use crate::{ActError, Result, ShareLock, Workflow};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, RwLock},
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

    pub fn build(workflow: &mut Workflow) -> Result<NodeTree> {
        let mut tree = NodeTree::new();
        build::build_workflow(workflow, &mut tree)?;

        Ok(tree)
    }

    pub fn load(&mut self, model: &Workflow) -> Result<()> {
        let mut model = model.clone();
        build::build_workflow(&mut model, self)
    }

    pub fn make(&self, id: &str, data: NodeContent, level: usize) -> Result<Arc<Node>> {
        let node = Arc::new(Node::new(id, data, level));

        let mut node_map = self.node_map.write().unwrap();
        if node_map.contains_key(node.id()) {
            return Err(ActError::Model(format!("dup node id with '{}'", node.id())));
        }
        node_map.insert(node.id().to_string(), node.clone());

        Ok(node)
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
        if let Some(ref root) = self.root.clone() {
            VisitRoot::walk(root, &move |node| {
                if node.level > 1 {
                    // level start from 1
                    // level 0 is the workflow itself
                    let mut level = 1;
                    while level < node.level {
                        if node.is_sibling(&level) {
                            print!("│ ");
                        } else {
                            print!("  ");
                        }

                        level += 1;
                    }
                }

                if node.is_next_sibling() {
                    print!("├─");
                } else {
                    if node.level != 0 {
                        print!("└─");
                    }
                }
                let next = match node.next().upgrade() {
                    Some(n) => n.id().to_string(),
                    None => "nil".to_string(),
                };

                println!(
                    "{} id:{} name={}  next={}",
                    node.r#type(),
                    node.id(),
                    node.name(),
                    next,
                );
            });
        }
    }

    #[allow(unused)]
    pub fn tree_output(&self) -> String {
        let s = &RefCell::new(String::new());
        if let Some(ref root) = self.root.clone() {
            VisitRoot::walk(root, &move |n| {
                if n.level > 1 {
                    let mut level = 0;
                    while level < n.level {
                        if n.is_sibling(&level) {
                            s.borrow_mut().push_str(&format!("{}", "│ "));
                        } else {
                            s.borrow_mut().push_str(&format!("{}", "  "));
                        }

                        level += 1;
                    }
                }

                if n.is_next_sibling() {
                    s.borrow_mut().push_str(&format!("{}", "├─"));
                } else {
                    if n.level != 0 {
                        s.borrow_mut().push_str(&format!("{}", "└─"));
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

    pub fn set_error(&mut self, err: ActError) {
        self.error = Some(err);
    }
}
