use super::{collect::Collect, database::Database};
use crate::store::{data::*, DbSet, StoreAdapter};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct LocalStore {
    db: Arc<RwLock<Database>>,
    models: Arc<Collect<Model>>,
    procs: Arc<Collect<Proc>>,
    tasks: Arc<Collect<Task>>,
    packages: Arc<Collect<Package>>,
    messages: Arc<Collect<Message>>,
}

impl LocalStore {
    pub fn new(path: &str, name: &str) -> Self {
        let db = Arc::new(RwLock::new(Database::new(path, name)));
        let models = Collect::new(&db, "models");
        let procs = Collect::new(&db, "procs");
        let tasks = Collect::new(&db, "tasks");
        let packages = Collect::new(&db, "packages");
        let messages = Collect::new(&db, "messages");
        let store = Self {
            db: db.clone(),
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

impl StoreAdapter for LocalStore {
    fn init(&self) {}
    fn close(&self) {
        self.db.write().unwrap().close();
    }

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
