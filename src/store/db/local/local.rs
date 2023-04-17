use super::{collect::Collect, database::Database};
use crate::store::{data::*, DbSet, StoreAdapter};
use once_cell::sync::OnceCell;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct LocalStore {
    db: Arc<RwLock<Database>>,
    models: Arc<Collect<Model>>,
    procs: Arc<Collect<Proc>>,
    tasks: Arc<Collect<Task>>,
    messages: Arc<Collect<Message>>,
}

static DB: OnceCell<Arc<RwLock<Database>>> = OnceCell::new();
const PATH: &str = "data";

impl LocalStore {
    pub fn new() -> Self {
        let db = DB.get_or_init(|| Arc::new(RwLock::new(Database::new(PATH))));
        let models = Collect::new(&db, "model");
        let procs = Collect::new(&db, "proc");
        let tasks = Collect::new(&db, "task");
        let messages = Collect::new(&db, "message");
        let store = Self {
            db: db.clone(),
            models: Arc::new(models),
            procs: Arc::new(procs),
            tasks: Arc::new(tasks),
            messages: Arc::new(messages),
        };

        store.init();

        store
    }
}

impl StoreAdapter for LocalStore {
    fn init(&self) {}
    fn flush(&self) {
        self.db.read().unwrap().flush();
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

    fn messages(&self) -> Arc<dyn DbSet<Item = Message>> {
        self.messages.clone()
    }
}
