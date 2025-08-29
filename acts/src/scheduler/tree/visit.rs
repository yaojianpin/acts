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
        let mut root = Visitor::new(&root, node, 0, 0, true, &HashMap::new());
        root.walk(f);
    }

    pub fn visit_count(&self, id: &str) -> usize {
        self.visits.get(id).copied().unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct Visitor {
    root: Box<VisitRoot>,
    pub level: usize,
    pub is_last: bool,
    pub index: usize,
    node: Arc<Node>,
    pub path: HashMap<usize, bool>,
}

impl Deref for Visitor {
    type Target = Arc<Node>;
    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

#[allow(clippy::borrowed_box)]
impl Visitor {
    pub fn new(
        root: &Box<VisitRoot>,
        node: &Arc<Node>,
        level: usize,
        index: usize,
        is_last: bool,
        path: &HashMap<usize, bool>,
        // next_sibling: bool,
    ) -> Box<Self> {
        let mut path = path.clone();
        path.entry(node.level)
            .and_modify(|v| *v = !is_last)
            .or_insert(!is_last);
        Box::new(Self {
            root: root.clone(),
            node: node.clone(),
            level,
            index,
            is_last,
            path,
        })
    }

    #[allow(clippy::vec_box)]
    pub fn children_visits(&self) -> Vec<Box<Self>> {
        let len = self.node.children().len();
        self.node
            .children()
            .iter()
            .enumerate()
            .map(|(i, iter)| {
                let mut is_last = i == len - 1;
                if iter.kind() == NodeKind::Step
                    && let Some(next) = iter.next().upgrade()
                    && self.root.visit_count(next.id()) == 0
                {
                    is_last = false;
                }
                Visitor::new(&self.root, iter, iter.level, i, is_last, &self.path)
            })
            .collect::<Vec<_>>()
    }

    pub fn next_visit(&self) -> Option<Box<Self>> {
        if let Some(next) = self.node.next().upgrade()
            && self.root.visit_count(next.id()) == 0
        {
            let node = Visitor::new(
                &self.root,
                &next,
                next.level,
                self.index + 1,
                next.next().upgrade().is_none(),
                &self.path,
            );
            Some(node)
        } else {
            None
        }
    }

    pub fn visit(&mut self) {
        self.root
            .visits
            .entry(self.node.id().to_string())
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
