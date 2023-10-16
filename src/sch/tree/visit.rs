use super::Node;
use crate::NodeKind;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

#[derive(Clone)]
pub struct VisitRoot {
    visits: HashMap<String, usize>,
}

impl VisitRoot {
    pub fn walk<F: Fn(&Visitor) + Clone>(node: &Arc<Node>, f: &F) {
        let root = Box::new(VisitRoot {
            visits: HashMap::new(),
        });
        let mut root = Visitor::new(&root, node, &HashMap::new(), false);
        root.walk(f);
    }

    pub fn visit_count(&self, id: &str) -> usize {
        self.visits.get(id).map(|v| *v).unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct Visitor {
    root: Box<VisitRoot>,
    next_sibling: bool,
    node: Arc<Node>,
    path: HashMap<usize, bool>,
}

impl Deref for Visitor {
    type Target = Arc<Node>;
    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

impl Visitor {
    pub fn new(
        root: &Box<VisitRoot>,
        node: &Arc<Node>,
        path: &HashMap<usize, bool>,
        next_sibling: bool,
    ) -> Box<Self> {
        let mut path = path.clone();
        path.entry(node.level)
            .and_modify(|v| *v = next_sibling)
            .or_insert(next_sibling);
        Box::new(Self {
            root: root.clone(),
            node: node.clone(),
            next_sibling,
            path,
        })
    }

    pub fn children_visits(&self) -> Vec<Box<Self>> {
        let len = self.node.children().len();
        self.node
            .children()
            .iter()
            .enumerate()
            .map(|(i, iter)| {
                let mut is_sibling = i < len - 1;
                if iter.kind() == NodeKind::Step {
                    is_sibling = false;
                    if let Some(next) = iter.next().upgrade() {
                        if self.root.visit_count(&next.id()) == 0 {
                            is_sibling = true
                        }
                    }
                }
                Visitor::new(&self.root, iter, &self.path, is_sibling)
            })
            .collect::<Vec<_>>()
    }

    pub fn next_visit(&self) -> Option<Box<Self>> {
        if let Some(next) = self.node.next().upgrade() {
            if self.root.visit_count(&next.id()) == 0 {
                let node = Visitor::new(
                    &self.root,
                    &next,
                    &self.path,
                    next.next().upgrade().is_some(),
                );
                return Some(node);
            }
        }

        None
    }

    pub fn next_sibling(&self) -> bool {
        self.next_sibling
    }

    pub fn is_sibling(&self, level: &usize) -> bool {
        self.path.get(level).unwrap_or(&false).clone()
    }

    pub fn visit(&mut self) {
        self.path.insert(self.node.level, self.next_sibling);
        self.root
            .visits
            .entry(self.node.id())
            .and_modify(|v| *v += 1)
            .or_insert(1);
    }

    pub fn walk<F: Fn(&Visitor) + Clone>(&mut self, f: &F) {
        f(self);
        self.visit();

        for node in self.children_visits().iter_mut() {
            node.walk(f);
        }

        if let Some(mut next) = self.next_visit() {
            next.walk(f);
        }
    }
}
