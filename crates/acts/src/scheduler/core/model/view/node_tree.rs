use super::{
    build,
    node::{Node, NodeContent, NodeOutputKind},
};
use crate::{ActError, Result, Workflow};

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct NodeTree {
    pub(crate) root: Option<Arc<Node>>,
    pub(crate) node_map: Arc<RwLock<HashMap<String, Arc<Node>>>>,
    pub(crate) model: Arc<Workflow>,
}

impl NodeTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn root(&self) -> Option<Arc<Node>> {
        self.root.clone()
    }

    pub fn load(&mut self, model: &Workflow) -> Result<()> {
        self.load_owned(model.clone())
    }

    pub(crate) fn load_owned(&mut self, model: Workflow) -> Result<()> {
        let mut on_ids = HashSet::new();

        // an `rn` is a literal resource name acl patterns match against, so
        // it may not be a glob, contain spaces, or carry empty segments —
        // a malformed one would silently match nothing
        if !model.rn.is_empty() {
            let rn = model.rn.trim();
            if rn.contains(|c: char| c.is_whitespace())
                || rn
                    .chars()
                    .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}'))
                || rn.split(':').any(str::is_empty)
            {
                return Err(ActError::Model(format!(
                    "invalid rn '{}' in model '{}': colon-separated segments, no spaces, no glob",
                    model.rn, model.id
                )));
            }
        }

        for on in model.on.iter() {
            // validate trigger declarations (id/kind/config) — triggers are
            // not process nodes, so nothing is inserted into the tree
            on.valid()?;
            if !on_ids.insert(on.id.as_str()) {
                return Err(ActError::Model(format!("dup event id with '{}'", on.id)));
            }
        }

        build::build_owned_workflow(model, self)
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

    pub fn set_root(&mut self, node: &Arc<Node>) {
        self.root = Some(node.clone());
    }

    pub fn node(&self, key: &str) -> Option<Arc<Node>> {
        let map = self.node_map.read();
        map.get(key).cloned()
    }

    pub fn tree_output(&self) -> String {
        let mut output = String::new();
        if let Some(root) = &self.root {
            let mut visited = HashSet::new();
            write_tree_output(root, true, &HashMap::new(), &mut visited, &mut output);
        }
        output
    }
}

fn write_tree_output(
    node: &Arc<Node>,
    is_last: bool,
    parent_path: &HashMap<usize, bool>,
    visited: &mut HashSet<String>,
    output: &mut String,
) {
    if !visited.insert(node.id().to_string()) {
        return;
    }

    let mut path = parent_path.clone();
    path.insert(node.level, !is_last);

    if node.level > 0 {
        for index in 1..node.level {
            if path.get(&index).copied().unwrap_or(false) {
                output.push_str("│   ");
            } else {
                output.push_str("    ");
            }
        }
        output.push_str(if is_last { "└── " } else { "├── " });
    }

    let next = match node.next().upgrade() {
        Some(next) => next.id().to_string(),
        None => "nil".to_string(),
    };
    output.push_str(&format!(
        "{} id:{} name={}  next={}\n",
        node.kind(),
        node.id(),
        node.name(),
        next
    ));

    for typ in [
        NodeOutputKind::Normal,
        NodeOutputKind::Catch,
        NodeOutputKind::Timeout,
    ] {
        let children = node.children_in(typ);
        let len = children.len();
        for (index, child) in children.iter().enumerate() {
            let mut child_is_last = index == len - 1;
            if let Some(next) = child.next().upgrade()
                && !visited.contains(next.id())
            {
                child_is_last = false;
            }
            write_tree_output(child, child_is_last, &path, visited, output);
        }
    }

    if let Some(next) = node.next().upgrade()
        && !visited.contains(next.id())
    {
        write_tree_output(
            &next,
            next.next().upgrade().is_none(),
            &path,
            visited,
            output,
        );
    }
}
