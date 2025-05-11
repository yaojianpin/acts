use crate::collection::{
    EventCollection, MessageCollection, ModelCollection, PackageCollection, ProcCollection,
    TaskCollection,
};
use acts::{DbCollection, data::*};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Error as DbError, Result as DbResult, Row};
use std::{fs, path::Path, sync::Arc};

pub trait DbRow {
    fn id(&self) -> &str;
    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
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
        let db_path = db_url.replace("sqlite://", "");

        let path = Path::new(&db_path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }

        let manager = SqliteConnectionManager::file(&db_path);
        let conn = r2d2::Pool::new(manager).unwrap();

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
