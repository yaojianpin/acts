use crate::{
    store::{none::NoneStore, Message, Model, Proc, StoreAdapter, Task},
    utils, ActError, ActResult, Engine, ShareLock, Workflow,
};
use std::sync::{Arc, Mutex, RwLock};
use tracing::trace;

#[cfg(feature = "sqlite")]
use crate::store::db::SqliteStore;

#[cfg(feature = "store")]
use crate::store::db::LocalStore;

#[derive(Clone, Debug, PartialEq)]
pub enum StoreKind {
    None,
    #[cfg(feature = "store")]
    Local,
    #[cfg(feature = "sqlite")]
    Sqlite,
    Extern,
}

pub struct Store {
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

    fn messages(&self) -> Arc<dyn super::DbSet<Item = Message>> {
        self.base.read().unwrap().messages()
    }

    fn flush(&self) {
        self.base.read().unwrap().flush()
    }
}

impl Store {
    pub fn new() -> Self {
        #[allow(unused_assignments)]
        let mut store: Arc<dyn StoreAdapter> = Arc::new(NoneStore::new());
        #[allow(unused_assignments)]
        let mut kind = StoreKind::None;

        #[cfg(feature = "sqlite")]
        {
            trace!("sqlite::new");
            store = SqliteStore::new();
            kind = StoreKind::Sqlite;
        }

        #[cfg(feature = "store")]
        {
            trace!("store::new");
            store = Arc::new(LocalStore::new());
            kind = StoreKind::Local;
        }

        Self {
            kind: Arc::new(Mutex::new(kind)),
            base: Arc::new(RwLock::new(store)),
        }
    }

    pub async fn init(&self, engine: &Engine) {
        trace!("store::init");

        if let Some(store) = engine.adapter().store() {
            *self.kind.lock().unwrap() = StoreKind::Extern;
            *self.base.write().unwrap() = store;
        }
    }

    pub fn deploy(&self, model: &Workflow) -> ActResult<bool> {
        trace!("store::create_model({})", model.id);
        if model.id.is_empty() {
            return Err(ActError::OperateError("missing id in model".into()));
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
                };
                models.create(&data)
            }
        }
    }

    fn base(&self) -> Arc<dyn StoreAdapter> {
        self.base.read().unwrap().clone()
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> StoreKind {
        self.kind.lock().unwrap().clone()
    }
}
