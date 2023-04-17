use crate::{
    store::{
        data::{Message, Model, Proc, Task},
        DbSet, Query, StoreAdapter,
    },
    ActResult,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct NoneStore {
    models: Collect<Model>,
    procs: Collect<Proc>,
    tasks: Collect<Task>,
    messages: Collect<Message>,
}

impl NoneStore {
    pub fn new() -> Self {
        Self {
            models: Collect::new(),
            procs: Collect::new(),
            tasks: Collect::new(),
            messages: Collect::new(),
        }
    }
}

impl StoreAdapter for NoneStore {
    fn init(&self) {}
    fn flush(&self) {}

    fn models(&self) -> Arc<dyn DbSet<Item = Model>> {
        Arc::new(self.models.clone())
    }

    fn procs(&self) -> Arc<dyn DbSet<Item = Proc>> {
        Arc::new(self.procs.clone())
    }

    fn tasks(&self) -> Arc<dyn DbSet<Item = Task>> {
        Arc::new(self.tasks.clone())
    }

    fn messages(&self) -> Arc<dyn DbSet<Item = Message>> {
        Arc::new(self.messages.clone())
    }
}

#[derive(Debug, Clone)]
pub struct Collect<T> {
    _data: Vec<T>,
}

impl<T> Collect<T> {
    pub fn new() -> Self {
        Self { _data: Vec::new() }
    }
}

impl<T> DbSet for Collect<T>
where
    T: Send + Sync,
{
    type Item = T;
    fn exists(&self, _id: &str) -> ActResult<bool> {
        Ok(false)
    }

    fn find(&self, _id: &str) -> ActResult<Self::Item> {
        Err(crate::ActError::StoreError(format!(
            "not found model id={}",
            _id
        )))
    }

    fn query(&self, _q: &Query) -> ActResult<Vec<Self::Item>> {
        Ok(vec![])
    }

    fn create(&self, _data: &Self::Item) -> ActResult<bool> {
        Ok(false)
    }
    fn update(&self, _data: &Self::Item) -> ActResult<bool> {
        Ok(false)
    }
    fn delete(&self, _id: &str) -> ActResult<bool> {
        Ok(false)
    }
}
