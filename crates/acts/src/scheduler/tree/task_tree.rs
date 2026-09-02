use crate::{NodeKind, scheduler::Task};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub struct TaskTree {
    maps: BTreeMap<String, Arc<Task>>,
    root: Option<Arc<Task>>,
    /// task instances created per node id; bounds re-execution of a node
    /// (a self-loop / cyclic `next` must not create tasks forever)
    run_counts: BTreeMap<String, usize>,
}

impl TaskTree {
    pub fn new() -> Self {
        Self {
            maps: BTreeMap::new(),
            root: None,
            run_counts: BTreeMap::new(),
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

    pub fn push(&mut self, task: Arc<Task>) {
        let is_new = !self.maps.contains_key(&task.id);
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
        if is_new {
            *self
                .run_counts
                .entry(task.node().id().to_string())
                .or_insert(0) += 1;
        }
        if task.node().kind() == NodeKind::Workflow {
            self.root = Some(task);
        }
    }
    /// number of task instances created for the node in this process
    pub fn run_count(&self, node_id: &str) -> usize {
        self.run_counts.get(node_id).copied().unwrap_or(0)
    }
}
