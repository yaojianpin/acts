use crate::{
    store::{db::LocalStore, Act, Model, Proc, StoreAdapter, Task},
    utils, ActError, ActResult, Engine, ShareLock, Workflow,
};
use std::sync::{Arc, Mutex, RwLock};
use tracing::trace;

#[cfg(feature = "sqlite")]
use crate::store::db::SqliteStore;

#[derive(Clone, Debug, PartialEq)]
pub enum StoreKind {
    Local,
    #[cfg(feature = "sqlite")]
    Sqlite,
    Extern,
}

pub struct Store {
    #[cfg(test)]
    path: String,
    kind: Arc<Mutex<StoreKind>>,
    base: ShareLock<Arc<dyn StoreAdapter>>,
}

impl StoreAdapter for Store {
    fn init(&self) {
        self.base.read().unwrap().init()
    }

    fn models(&self) -> Arc<dyn super::DbSet<Item = Model>> {
        self.base.read().unwrap().models()
    }

    fn procs(&self) -> Arc<dyn super::DbSet<Item = Proc>> {
        self.base.read().unwrap().procs()
    }

    fn tasks(&self) -> Arc<dyn super::DbSet<Item = Task>> {
        self.base.read().unwrap().tasks()
    }

    fn acts(&self) -> Arc<dyn super::DbSet<Item = Act>> {
        self.base.read().unwrap().acts()
    }

    fn flush(&self) {
        self.base.read().unwrap().flush()
    }
}

impl Store {
    pub fn new() -> Self {
        Self::new_with_path("data")
    }

    pub fn new_with_path(path: &str) -> Self {
        let (store, kind) = create_default_store(path);
        Self {
            #[cfg(test)]
            path: path.to_string(),
            kind: Arc::new(Mutex::new(kind)),
            base: Arc::new(RwLock::new(store)),
        }
    }

    pub fn init(&self, engine: &Engine) {
        trace!("store::init");
        if let Some(store) = engine.adapter().store() {
            *self.kind.lock().unwrap() = StoreKind::Extern;
            *self.base.write().unwrap() = store;
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        let (store, kind) = create_default_store(&self.path);
        *self.kind.lock().unwrap() = kind;
        *self.base.write().unwrap() = store;
    }

    pub fn deploy(&self, model: &Workflow) -> ActResult<bool> {
        trace!("store::create_model({})", model.id);
        if model.id.is_empty() {
            return Err(ActError::Action("missing id in model".into()));
        }
        let models = self.base().models();
        match models.find(&model.id) {
            Ok(m) => {
                let text = serde_yaml::to_string(model).unwrap();
                let data = Model {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    model: text.clone(),
                    ver: m.ver + 1,
                    size: text.len() as u32,
                    time: utils::time::time(),
                    topic: m.topic.clone(),
                };
                models.update(&data)
            }
            Err(_) => {
                let text = serde_yaml::to_string(model).unwrap();
                let data = Model {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    model: text.clone(),
                    ver: 1,
                    size: text.len() as u32,
                    time: utils::time::time(),
                    topic: model.topic.clone(),
                };
                models.create(&data)
            }
        }
    }

    fn base(&self) -> Arc<dyn StoreAdapter> {
        self.base.read().unwrap().clone()
    }

    pub fn kind(&self) -> StoreKind {
        self.kind.lock().unwrap().clone()
    }
}

fn create_default_store(path: &str) -> (Arc<dyn StoreAdapter + 'static>, StoreKind) {
    #[allow(unused_mut)]
    let mut store = Arc::new(LocalStore::new(path));
    #[allow(unused_mut)]
    let mut kind = StoreKind::Local;

    #[cfg(feature = "sqlite")]
    {
        trace!("sqlite::new");
        store = Arc::new(SqliteStore::new());
        kind = StoreKind::Sqlite;
    }

    (store, kind)
}
