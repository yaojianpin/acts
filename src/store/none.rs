use crate::{
    adapter::StoreAdapter,
    store::{DataSet, Message, Model, Proc, Query, Task},
    ActResult,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct NoneStore {
    models: ModelSet,
    procs: ProcSet,
    tasks: TaskSet,
    messages: MessageSet,
}

impl NoneStore {
    pub fn new() -> Self {
        Self {
            models: ModelSet,
            procs: ProcSet,
            tasks: TaskSet,
            messages: MessageSet,
        }
    }
}

impl StoreAdapter for NoneStore {
    fn init(&self) {}
    fn flush(&self) {}

    fn models(&self) -> Arc<dyn DataSet<Model>> {
        Arc::new(self.models.clone())
    }

    fn procs(&self) -> Arc<dyn DataSet<Proc>> {
        Arc::new(self.procs.clone())
    }

    fn tasks(&self) -> Arc<dyn DataSet<Task>> {
        Arc::new(self.tasks.clone())
    }

    fn messages(&self) -> Arc<dyn DataSet<Message>> {
        Arc::new(self.messages.clone())
    }
}

#[derive(Debug, Clone)]
pub struct ModelSet;

impl DataSet<Model> for ModelSet {
    fn exists(&self, _id: &str) -> bool {
        false
    }

    fn find(&self, _id: &str) -> Option<Model> {
        None
    }

    fn query(&self, _q: &Query) -> ActResult<Vec<Model>> {
        Ok(vec![])
    }

    fn create(&self, _data: &Model) -> ActResult<bool> {
        Ok(false)
    }
    fn update(&self, _data: &Model) -> ActResult<bool> {
        Ok(false)
    }
    fn delete(&self, _id: &str) -> ActResult<bool> {
        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct ProcSet;

impl DataSet<Proc> for ProcSet {
    fn exists(&self, _id: &str) -> bool {
        false
    }

    fn find(&self, _id: &str) -> Option<Proc> {
        None
    }

    fn query(&self, _q: &Query) -> ActResult<Vec<Proc>> {
        Ok(vec![])
    }

    fn create(&self, _data: &Proc) -> ActResult<bool> {
        Ok(false)
    }
    fn update(&self, _data: &Proc) -> ActResult<bool> {
        Ok(false)
    }
    fn delete(&self, _id: &str) -> ActResult<bool> {
        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct TaskSet;

impl DataSet<Task> for TaskSet {
    fn exists(&self, _id: &str) -> bool {
        false
    }

    fn find(&self, _id: &str) -> Option<Task> {
        None
    }

    fn query(&self, _q: &Query) -> ActResult<Vec<Task>> {
        Ok(vec![])
    }

    fn create(&self, _data: &Task) -> ActResult<bool> {
        Ok(false)
    }
    fn update(&self, _data: &Task) -> ActResult<bool> {
        Ok(false)
    }
    fn delete(&self, _id: &str) -> ActResult<bool> {
        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct MessageSet;

impl DataSet<Message> for MessageSet {
    fn exists(&self, _id: &str) -> bool {
        false
    }

    fn find(&self, _id: &str) -> Option<Message> {
        None
    }

    fn query(&self, _q: &Query) -> ActResult<Vec<Message>> {
        Ok(vec![])
    }

    fn create(&self, _data: &Message) -> ActResult<bool> {
        Ok(false)
    }
    fn update(&self, _data: &Message) -> ActResult<bool> {
        Ok(false)
    }
    fn delete(&self, _id: &str) -> ActResult<bool> {
        Ok(false)
    }
}
