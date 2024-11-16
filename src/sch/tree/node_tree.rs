use super::{
    build,
    node::{Node, NodeContent},
    visit::VisitRoot,
};
use crate::{ActError, Result, ShareLock, Workflow};
use std::{cell::RefCell, collections::HashMap, sync::Arc};

#[derive(Default, Clone)]
pub struct NodeTree {
    pub(crate) root: Option<Arc<Node>>,
    pub(crate) node_map: ShareLock<HashMap<String, Arc<Node>>>,
    pub(crate) error: Option<ActError>,
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
        map.get(key).cloned()
    }

    #[allow(unused)]
    pub fn print(&self) {
        if let Some(ref root) = self.root.clone() {
            VisitRoot::walk(root, &move |node| {
                // print single line
                for index in 0..node.level {
                    if index == 0 {
                        print!("│   ");
                        // write!(f, "    ")?;
                    } else {
                        print!("    ");
                        // write!(f, "│   ")?;
                    }
                }
                if node.is_last {
                    print!("└── ");
                    // writeln!(f, "└── {}", leaf.0)?;
                } else {
                    print!("├── ");
                    // writeln!(f, "├── {}", leaf.0)?;
                }
                let next = match node.next().upgrade() {
                    Some(n) => n.id().to_string(),
                    None => "nil".to_string(),
                };

                println!(
                    "{} id:{} name={}  next={}",
                    node.typ(),
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
                // print single line
                for index in 0..n.level {
                    if n.path[&index] {
                        s.borrow_mut().push_str("│   ");
                        // write!(f, "    ")?;
                    } else {
                        s.borrow_mut().push_str("    ");
                        // write!(f, "│   ")?;
                    }
                }
                if n.is_last {
                    s.borrow_mut().push_str("└── ");
                    // writeln!(f, "└── {}", leaf.0)?;
                } else {
                    s.borrow_mut().push_str("├── ");
                    // writeln!(f, "├── {}", leaf.0)?;
                }

                // if n.level > 1 {
                //     let mut level = 0;
                //     while level < n.level {
                //         if n.is_sibling(&level) {
                //             s.borrow_mut().push_str("│ ");
                //         } else {
                //             s.borrow_mut().push_str("  ");
                //         }

                //         level += 1;
                //     }
                // }

                // if n.is_next_sibling() {
                //     s.borrow_mut().push_str("├─");
                // } else if n.level != 0 {
                //     s.borrow_mut().push_str("└─");
                // }

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
