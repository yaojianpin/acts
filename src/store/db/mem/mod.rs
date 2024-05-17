mod collect;
mod r#impl;

use crate::{
    store::{data::*, DbSet, StoreAdapter},
    Result,
};
use collect::Collect;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value as JsonValue;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone)]
pub struct MemStore {
    models: Arc<Collect<Model>>,
    procs: Arc<Collect<Proc>>,
    tasks: Arc<Collect<Task>>,
    packages: Arc<Collect<Package>>,
    messages: Arc<Collect<Message>>,
}

trait DbDocument: Serialize + DeserializeOwned {
    fn id(&self) -> &str;
    fn doc(&self) -> Result<HashMap<String, JsonValue>>;
}

impl MemStore {
    pub fn new() -> Self {
        let models = Collect::new("models");
        let procs = Collect::new("procs");
        let tasks = Collect::new("tasks");
        let packages = Collect::new("packages");
        let messages = Collect::new("messages");
        let store = Self {
            models: Arc::new(models),
            procs: Arc::new(procs),
            tasks: Arc::new(tasks),
            packages: Arc::new(packages),
            messages: Arc::new(messages),
        };

        store.init();

        store
    }
}

impl StoreAdapter for MemStore {
    fn init(&self) {}
    fn close(&self) {}

    fn models(&self) -> Arc<dyn DbSet<Item = Model>> {
        self.models.clone()
    }

    fn procs(&self) -> Arc<dyn DbSet<Item = Proc>> {
        self.procs.clone()
    }

    fn tasks(&self) -> Arc<dyn DbSet<Item = Task>> {
        self.tasks.clone()
    }

    fn packages(&self) -> Arc<dyn DbSet<Item = Package>> {
        self.packages.clone()
    }

    fn messages(&self) -> Arc<dyn DbSet<Item = Message>> {
        self.messages.clone()
    }
}
