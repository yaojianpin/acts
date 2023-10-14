use crate::{sch::Task, NodeKind};
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub struct TaskTree {
    maps: HashMap<String, Arc<Task>>,
    root: Option<Arc<Task>>,
}

impl TaskTree {
    pub fn new() -> Self {
        Self {
            maps: HashMap::new(),
            root: None,
        }
    }

    pub fn tasks(&self) -> Vec<Arc<Task>> {
        self.maps.values().cloned().collect()
    }

    pub fn root(&self) -> Option<Arc<Task>> {
        self.root.clone()
    }

    pub fn task_by_tid(&self, tid: &str) -> Option<Arc<Task>> {
        self.maps.get(tid).map(|t| t.clone())
    }

    pub fn find_tasks(&self, predicate: impl Fn(&Arc<Task>) -> bool) -> Vec<Arc<Task>> {
        let mut tasks = Vec::new();

        for (_, t) in &self.maps {
            if predicate(t) {
                tasks.push(t.clone());
            }
        }
        tasks
    }

    pub fn push(&mut self, task: Arc<Task>) {
        self.maps.insert(task.id.clone(), task.clone());
        if task.node.kind() == NodeKind::Workflow {
            self.root = Some(task);
        }
    }
}
