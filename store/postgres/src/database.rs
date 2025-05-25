use super::synclient::SynClient;
use crate::collection::{
    EventCollection, MessageCollection, ModelCollection, PackageCollection, ProcCollection,
    TaskCollection,
};
use acts::{DbCollection, data::*};
use sqlx::{Error as DbError, postgres::PgRow};
use std::sync::Arc;

pub trait DbRow {
    fn id(&self) -> &str;
    fn from_row(row: &PgRow) -> std::result::Result<Self, DbError>
    where
        Self: Sized;
}

pub trait DbInit {
    fn init(&self);
}

pub struct Database {
    models: Arc<ModelCollection>,
    procs: Arc<ProcCollection>,
    tasks: Arc<TaskCollection>,
    packages: Arc<PackageCollection>,
    messages: Arc<MessageCollection>,
    events: Arc<EventCollection>,
}

impl Database {
    pub fn new(db_url: &str) -> Self {
        let conn = Arc::new(SynClient::connect(db_url));
        let models = ModelCollection::new(&conn);
        let procs = ProcCollection::new(&conn);
        let tasks = TaskCollection::new(&conn);
        let packages = PackageCollection::new(&conn);
        let messages = MessageCollection::new(&conn);
        let events = EventCollection::new(&conn);

        Self {
            models: Arc::new(models),
            procs: Arc::new(procs),
            tasks: Arc::new(tasks),
            packages: Arc::new(packages),
            messages: Arc::new(messages),
            events: Arc::new(events),
        }
    }

    pub fn tasks(&self) -> Arc<dyn DbCollection<Item = Task> + Send + Sync> {
        self.tasks.clone()
    }

    pub fn procs(&self) -> Arc<dyn DbCollection<Item = Proc> + Send + Sync> {
        self.procs.clone()
    }

    pub fn packages(&self) -> Arc<dyn DbCollection<Item = Package> + Send + Sync> {
        self.packages.clone()
    }

    pub fn models(&self) -> Arc<dyn DbCollection<Item = Model> + Send + Sync> {
        self.models.clone()
    }

    pub fn messages(&self) -> Arc<dyn DbCollection<Item = Message> + Send + Sync> {
        self.messages.clone()
    }

    pub fn events(&self) -> Arc<dyn DbCollection<Item = Event> + Send + Sync> {
        self.events.clone()
    }

    pub fn init(&self) {
        self.packages.init();
        self.models.init();
        self.procs.init();
        self.tasks.init();
        self.messages.init();
        self.events.init();
    }
}
