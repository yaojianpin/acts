use crate::{
    store::{Act, DbSet, Model, Proc, Query, StoreAdapter, StoreKind, Task},
    ActResult, Engine,
};
use std::sync::Arc;

#[tokio::test]
async fn adapter_set_extern_store_test() {
    let engine = Engine::new();
    let store = TestStore::new();
    engine.adapter().set_extern_store(&store);
    engine.start();

    assert_eq!(engine.store().kind(), StoreKind::Extern);
    engine.store().reset();
}

#[derive(Debug, Clone)]
pub struct TestStore {
    models: Collect<Model>,
    procs: Collect<Proc>,
    tasks: Collect<Task>,
    acts: Collect<Act>,
}

impl TestStore {
    pub fn new() -> Self {
        Self {
            models: Collect::new(),
            procs: Collect::new(),
            tasks: Collect::new(),
            acts: Collect::new(),
        }
    }
}

impl StoreAdapter for TestStore {
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

    fn acts(&self) -> Arc<dyn DbSet<Item = Act>> {
        Arc::new(self.acts.clone())
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
        Err(crate::ActError::Store(format!(
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
