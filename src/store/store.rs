use crate::{
    store::{db::LocalStore, Model, Proc, StoreAdapter, Task},
    utils, ActError, Result, ShareLock, Workflow,
};
use std::sync::{Arc, Mutex, RwLock};
use tracing::trace;

use super::db::MemStore;

#[derive(Clone, Debug, PartialEq)]
pub enum StoreKind {
    Memory,
    Local,
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

    fn flush(&self) {
        self.base.read().unwrap().flush()
    }
}

impl Store {
    pub fn default() -> Arc<Self> {
        let store = Arc::new(MemStore::new());
        Arc::new(Self {
            kind: Arc::new(Mutex::new(StoreKind::Memory)),
            base: Arc::new(RwLock::new(store)),
        })
    }

    pub fn create(store: Arc<dyn StoreAdapter + 'static>) -> Self {
        Self {
            kind: Arc::new(Mutex::new(StoreKind::Extern)),
            base: Arc::new(RwLock::new(store)),
        }
    }

    pub fn local(path: &str) -> Self {
        let store = Arc::new(LocalStore::new(path));
        Self {
            kind: Arc::new(Mutex::new(StoreKind::Local)),
            base: Arc::new(RwLock::new(store)),
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        let store = Self::default();
        #[cfg(feature = "local_store")]
        let store = Self::local("data");
        *self.kind.lock().unwrap() = store.kind();
        *self.base.write().unwrap() = store.base();
    }

    pub fn deploy(&self, model: &Workflow) -> Result<bool> {
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
                    data: text.clone(),
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
                    data: text.clone(),
                    ver: 1,
                    size: text.len() as u32,
                    time: utils::time::time(),
                };
                models.create(&data)
            }
        }
    }

    pub(crate) fn base(&self) -> Arc<dyn StoreAdapter> {
        self.base.read().unwrap().clone()
    }

    pub fn kind(&self) -> StoreKind {
        self.kind.lock().unwrap().clone()
    }
}
