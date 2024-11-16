use crate::{
    store::{data, DbSet, Query, StoreAdapter, StoreKind},
    Builder, Result,
};
use std::sync::Arc;
use tokio::sync::OnceCell;

static STORE: OnceCell<TestStore> = OnceCell::const_new();
async fn init() -> TestStore {
    
    TestStore::new()
}

async fn store() -> &'static TestStore {
    STORE.get_or_init(init).await
}

#[tokio::test]
async fn adapter_set_extern_store_test() {
    let store = store().await;
    let engine = Builder::new().store(store).build();
    let store = engine.runtime().cache().store();
    assert_eq!(store.kind(), StoreKind::Extern);
    store.reset();
}

#[derive(Debug, Clone)]
pub struct TestStore {
    models: Collect<data::Model>,
    procs: Collect<data::Proc>,
    tasks: Collect<data::Task>,
    packages: Collect<data::Package>,
    messages: Collect<data::Message>,
}

impl TestStore {
    pub fn new() -> Self {
        Self {
            models: Collect::new(),
            procs: Collect::new(),
            tasks: Collect::new(),
            packages: Collect::new(),
            messages: Collect::new(),
        }
    }
}

impl StoreAdapter for TestStore {
    fn init(&self) {}
    fn close(&self) {}

    fn models(&self) -> Arc<dyn DbSet<Item = data::Model>> {
        Arc::new(self.models.clone())
    }

    fn procs(&self) -> Arc<dyn DbSet<Item = data::Proc>> {
        Arc::new(self.procs.clone())
    }

    fn tasks(&self) -> Arc<dyn DbSet<Item = data::Task>> {
        Arc::new(self.tasks.clone())
    }

    fn packages(&self) -> Arc<dyn DbSet<Item = data::Package>> {
        Arc::new(self.packages.clone())
    }

    fn messages(&self) -> Arc<dyn DbSet<Item = data::Message>> {
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
    fn exists(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }

    fn find(&self, _id: &str) -> Result<Self::Item> {
        Err(crate::ActError::Store(format!(
            "not found model id={}",
            _id
        )))
    }

    fn query(&self, _q: &Query) -> Result<Vec<Self::Item>> {
        Ok(vec![])
    }

    fn create(&self, _data: &Self::Item) -> Result<bool> {
        Ok(false)
    }
    fn update(&self, _data: &Self::Item) -> Result<bool> {
        Ok(false)
    }
    fn delete(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }
}
