use crate::{sch::Task, NodeKind};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub struct TaskTree {
    maps: BTreeMap<String, Arc<Task>>,
    root: Option<Arc<Task>>,
}

impl TaskTree {
    pub fn new() -> Self {
        Self {
            maps: BTreeMap::new(),
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
        self.maps
            .entry(task.id.clone())
            .and_modify(|t| {
                *t = task.clone();
                // t.set_pure_state(task.state());
                // t.set_end_time(task.end_time());
                // t.set_data(&task.data());
                // t.set_hooks(&task.hooks());
                // if let Some(err) = task.err() {
                //     t.set_err(&err);
                // }
            })
            .or_insert(task.clone());
        if task.node().kind() == NodeKind::Workflow {
            self.root = Some(task);
        }
    }
}
