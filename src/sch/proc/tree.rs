use crate::{sch::ActId, sch::Task, ShareLock, Workflow};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock, Weak},
};

#[derive(Clone)]
pub struct Node<T> {
    data: T,
    pub level: usize,
    parent: Arc<RwLock<Weak<Node<T>>>>,
    children: Arc<RwLock<Vec<Arc<Node<T>>>>>,
    next: Arc<RwLock<Weak<Node<T>>>>,
    //prev: Arc<RwLock<Weak<Node<T>>>>,
}

impl<T> Node<T>
where
    T: ActId,
{
    pub fn parent(&self) -> Option<Arc<Node<T>>> {
        let node = self.parent.read().unwrap();
        node.upgrade()
    }

    pub fn set_parent(&self, parent: &Arc<Node<T>>) {
        *self.parent.write().unwrap() = Arc::downgrade(&parent);
        parent
            .children
            .write()
            .unwrap()
            .push(Arc::new(self.clone()));
    }

    pub fn set_next(&self, node: &Arc<Node<T>>) {
        *self.next.write().unwrap() = Arc::downgrade(node);
    }

    pub fn children(&self) -> Vec<Arc<Node<T>>> {
        let node = self.children.read().unwrap();
        node.clone()
    }

    pub fn next(&self) -> Weak<Node<T>> {
        let next = self.next.read().unwrap();
        next.clone()
    }

    pub fn data(&self) -> T {
        self.data.clone()
    }

    pub fn id(&self) -> String {
        self.data.tid()
    }
}

impl<T> std::fmt::Debug for Node<T>
where
    T: std::fmt::Debug,
{
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

#[derive(Clone)]
pub struct Tree<T> {
    pub(crate) root: Option<Arc<Node<T>>>,
    pub(crate) node_map: ShareLock<HashMap<String, Arc<Node<T>>>>,
}

impl<T> Tree<T>
where
    T: ActId + Clone + std::fmt::Display,
{
    pub fn new() -> Self {
        Tree {
            root: None,
            node_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    pub fn make(&self, data: T, level: usize) -> Arc<Node<T>> {
        let node = Arc::new(Node {
            data: data,
            level,
            parent: Arc::new(RwLock::new(Weak::new())),
            children: Arc::new(RwLock::new(Vec::new())),
            //prev: Arc::new(RwLock::new(Weak::new())),
            next: Arc::new(RwLock::new(Weak::new())),
        });

        self.node_map
            .write()
            .unwrap()
            .insert(node.id(), node.clone());

        node
    }

    pub fn set_root(&mut self, node: &Arc<Node<T>>) {
        self.root = Some(node.clone());
    }

    pub fn push_act(&self, act: &T, parent_tid: &str) {
        let parent = self.node(parent_tid).unwrap();
        let node = self.make(act.clone() as T, parent.level + 1);
        node.set_parent(&parent);
        self.node_map.write().unwrap().insert(act.tid(), node);
    }

    pub fn node(&self, key: &str) -> Option<Arc<Node<T>>> {
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
                    print!("  ");
                    level -= 1;
                }
                print!("{}\n", node.data());
            });
        }
    }

    #[allow(unused)]
    pub fn walk<F: Fn(&Node<T>) + Clone>(&self, f: F) {
        if let Some(node) = self.root.clone() {
            self.visit_(&node, f);
        }
    }
    fn visit_<F: Fn(&Node<T>) + Clone>(&self, node: &Arc<Node<T>>, f: F) {
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

// pub fn from(job: &Job) -> Arc<Tree<Task>> {
//     let mut tree = Tree::new();

//     utils::process_job(&job, &mut tree);

//     Arc::new(tree)
// }

pub fn from(workflow: &mut Workflow) -> Arc<Tree<Task>> {
    let mut tree = Tree::new();
    utils::process_workflow(workflow, &mut tree);

    Arc::new(tree)
}

mod utils {
    use super::{Node, Tree};
    use crate::{
        sch::{Task, TaskState},
        utils::shortid,
        ActError, Branch, Job, Step, Workflow,
    };
    use std::sync::Arc;

    pub fn process_workflow(workflow: &mut Workflow, tree: &mut Tree<Task>) {
        let level = 0;
        if workflow.id.is_empty() {
            workflow.set_id(&shortid())
        }
        let task = Task::Workflow(workflow.id(), workflow.clone());

        let root = tree.make(task, level);
        tree.set_root(&root);

        let mut prev = root.clone();
        for job in workflow.jobs.iter_mut() {
            process_job(job, tree, &root, &mut prev, level + 1);
        }
    }

    pub fn process_job(
        job: &mut Job,
        tree: &mut Tree<Task>,
        parent: &Arc<Node<Task>>,
        prev: &mut Arc<Node<Task>>,
        level: usize,
    ) {
        if job.id.is_empty() {
            job.set_id(&shortid());
        }
        let task = Task::Job(job.id(), job.clone());
        let node = tree.make(task, level);
        if node.level == prev.level {
            prev.set_next(&node);
        }
        node.set_parent(&parent);

        let mut prev = node.clone();
        for step in job.steps.iter_mut() {
            process_step(step, tree, &node, &mut prev, level + 1);
        }
    }

    pub fn process_step(
        step: &mut Step,
        tree: &mut Tree<Task>,
        parent: &Arc<Node<Task>>,
        prev: &mut Arc<Node<Task>>,
        level: usize,
    ) {
        if step.id.is_empty() {
            step.set_id(&shortid());
        }
        let flow = Task::Step(step.id(), step.clone());
        let node = tree.make(flow, level);

        // only set the nodes in save level
        if node.level == prev.level {
            prev.set_next(&node);
        }
        node.set_parent(&parent);

        let next = step.next.clone();
        match next {
            Some(next_id) => {
                let flow_option = tree.node(&next_id);
                match flow_option {
                    Some(next) => {
                        *node.next.write().unwrap() = Arc::downgrade(&next);
                    }
                    None => step.set_state(&TaskState::Abort(ActError::NextError(next_id).into())),
                }
            }
            None => {
                if step.branches.len() > 0 {
                    let mut branch_prev = node.clone();
                    for branch in step.branches.iter_mut() {
                        process_branch(branch, tree, &node, &mut branch_prev, level + 1);
                    }
                }
            }
        }

        *prev = node.clone();
    }

    pub fn process_branch(
        branch: &mut Branch,
        tree: &mut Tree<Task>,
        parent: &Arc<Node<Task>>,
        prev: &mut Arc<Node<Task>>,
        level: usize,
    ) {
        if branch.id.is_empty() {
            branch.set_id(&shortid());
        }
        let flow = Task::Branch(branch.id(), branch.clone());
        let node = tree.make(flow, level);
        if node.level == prev.level {
            prev.set_next(&node);
        }
        prev.set_next(&node);
        node.set_parent(&parent);
        parent.children.write().unwrap().push(node.clone());

        let parent = node.clone();
        let mut step_prev = node.clone();
        for step in branch.steps.iter_mut() {
            process_step(step, tree, &parent, &mut step_prev, level + 1);
        }

        *prev = node.clone();
    }
}
