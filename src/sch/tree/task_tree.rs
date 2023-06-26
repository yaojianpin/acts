use crate::sch::Task;
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

    pub fn task_by_tid(&self, tid: &str) -> Option<Arc<Task>> {
        self.maps.get(tid).map(|t| t.clone())
        // match self.maps.get(tid) {
        //     Some(task) => Some(task.clone()),
        //     None => None,
        // }
    }

    pub fn find_next_tasks(&self, tid: &str) -> Vec<Arc<Task>> {
        let mut tasks = Vec::new();
        for (_, t) in &self.maps {
            if t.prev() == Some(tid.to_string()) {
                tasks.push(t.clone());
            }
        }

        tasks
    }

    pub fn task_by_nid(&self, nid: &str) -> Vec<Arc<Task>> {
        let mut tasks = Vec::new();
        for (_, t) in &self.maps {
            if t.node.id() == nid {
                tasks.push(t.clone());
            }
        }

        tasks
    }

    pub fn last_task_by_nid(&self, nid: &str) -> Option<Arc<Task>> {
        let mut tasks = self.task_by_nid(nid);
        tasks.sort_by(|a, b| b.end_time().cmp(&a.end_time()));

        tasks.first().cloned()
    }

    // pub fn task_by_uid(&self, uid: &str, state: TaskState) -> Vec<Arc<Task>> {
    //     let mut tasks = Vec::new();
    //     for (_, t) in &self.maps {
    //         if t.uid() == Some(uid.to_string()) && t.state() == state {
    //             tasks.push(t.clone());
    //         }
    //     }

    //     tasks
    // }

    pub fn push(&mut self, task: Arc<Task>) {
        self.maps.insert(task.tid.clone(), task.clone());
        if self.root.is_none() {
            self.root = Some(task);
        }
    }
}
