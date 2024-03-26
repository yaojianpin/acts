use crate::{
    store::{Model, Package, Proc, StoreAdapter, Task},
    utils, ActError, Result, ShareLock, Workflow,
};
use std::sync::{Arc, Mutex, RwLock};
use tracing::trace;

#[cfg(feature = "store")]
use crate::store::db::LocalStore;

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

    fn packages(&self) -> Arc<dyn super::DbSet<Item = Package>> {
        self.base.read().unwrap().packages()
    }

    fn close(&self) {
        self.base.read().unwrap().close()
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

    #[cfg(feature = "store")]
    pub fn local(path: &str, name: &str) -> Self {
        let store = Arc::new(LocalStore::new(path, name));
        Self {
            kind: Arc::new(Mutex::new(StoreKind::Local)),
            base: Arc::new(RwLock::new(store)),
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        #[cfg(not(feature = "store"))]
        {
            let store = Self::default();
            *self.kind.lock().unwrap() = store.kind();
            *self.base.write().unwrap() = store.base();
        }
        #[cfg(feature = "store")]
        {
            let store = Self::local("data", "acts.db");
            *self.kind.lock().unwrap() = store.kind();
            *self.base.write().unwrap() = store.base();
        }
    }

    pub fn publish(&self, pack: &Package) -> Result<bool> {
        trace!("store::publish({})", pack.id);
        if pack.id.is_empty() {
            return Err(ActError::Action("missing id in package".into()));
        }

        if pack.file_data.len() == 0 {
            return Err(ActError::Action("missing file in package".into()));
        }

        let packages = self.base().packages();
        match packages.find(&pack.id) {
            Ok(m) => {
                let data = Package {
                    create_time: m.create_time,
                    update_time: utils::time::time(),
                    ..pack.clone()
                };
                packages.update(&data)
            }
            Err(_) => {
                let data = Package {
                    create_time: utils::time::time(),
                    ..pack.clone()
                };
                packages.create(&data)
            }
        }
    }
    pub fn deploy(&self, model: &Workflow) -> Result<bool> {
        trace!("store::deploy({})", model.id);
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
