use crate::{NodeKind, Result, scheduler::shell::Task};
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

    pub fn task_by_tid(&self, tid: &str) -> Option<Arc<Task>> {
        self.maps.get(tid).cloned()
    }

    pub fn find_tasks(&self, predicate: impl Fn(&Arc<Task>) -> bool) -> Vec<Arc<Task>> {
        let mut tasks = Vec::new();
        for t in self.maps.values() {
            if predicate(t) {
                tasks.push(t.clone());
            }
        }
        tasks
    }

    pub fn push(&mut self, task: Arc<Task>) -> Result<()> {
        // only insert and not replace old task
        self.maps.entry(task.id.clone()).or_insert(task.clone());
        if task.node().kind() == NodeKind::Workflow {
            self.root = Some(task);
        }
        Ok(())
    }
}
