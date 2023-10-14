mod collect;

use crate::store::{data::*, DbSet, StoreAdapter};
use collect::Collect;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct MemStore {
    models: Arc<Collect<Model>>,
    procs: Arc<Collect<Proc>>,
    tasks: Arc<Collect<Task>>,
}

impl MemStore {
    pub fn new() -> Self {
        let models = Collect::new("models");
        let procs = Collect::new("procs");
        let tasks = Collect::new("tasks");
        let store = Self {
            models: Arc::new(models),
            procs: Arc::new(procs),
            tasks: Arc::new(tasks),
        };

        store.init();

        store
    }
}

impl StoreAdapter for MemStore {
    fn init(&self) {}
    fn flush(&self) {}

    fn models(&self) -> Arc<dyn DbSet<Item = Model>> {
        self.models.clone()
    }

    fn procs(&self) -> Arc<dyn DbSet<Item = Proc>> {
        self.procs.clone()
    }

    fn tasks(&self) -> Arc<dyn DbSet<Item = Task>> {
        self.tasks.clone()
    }
}
