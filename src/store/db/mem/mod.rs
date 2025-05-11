mod collect;
mod r#impl;

use crate::{
    Result, data,
    store::{DbCollection, data::*},
};
pub use collect::Collect;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone)]
pub struct MemStore {
    models: Arc<Collect<Model>>,
    procs: Arc<Collect<Proc>>,
    tasks: Arc<Collect<Task>>,
    packages: Arc<Collect<Package>>,
    messages: Arc<Collect<Message>>,
    events: Arc<Collect<Event>>,
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
        let events = Collect::new("events");

        let store = Self {
            models: Arc::new(models),
            procs: Arc::new(procs),
            tasks: Arc::new(tasks),
            packages: Arc::new(packages),
            messages: Arc::new(messages),
            events: Arc::new(events),
        };

        store
    }

    pub fn tasks(&self) -> Arc<dyn DbCollection<Item = data::Task> + Send + Sync> {
        self.tasks.clone()
    }

    pub fn procs(&self) -> Arc<dyn DbCollection<Item = data::Proc> + Send + Sync> {
        self.procs.clone()
    }

    pub fn packages(&self) -> Arc<dyn DbCollection<Item = data::Package> + Send + Sync> {
        self.packages.clone()
    }

    pub fn models(&self) -> Arc<dyn DbCollection<Item = data::Model> + Send + Sync> {
        self.models.clone()
    }

    pub fn messages(&self) -> Arc<dyn DbCollection<Item = data::Message> + Send + Sync> {
        self.messages.clone()
    }

    pub fn events(&self) -> Arc<dyn DbCollection<Item = data::Event> + Send + Sync> {
        self.events.clone()
    }
}
