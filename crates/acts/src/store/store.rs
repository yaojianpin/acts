use super::{DbCollection, DbCollectionIden, StoreIden, data, db::MemStore};
use crate::{
    ActError, Result, ShareLock, Workflow,
    store::{Model, Package},
    utils,
};
use std::{
    any::Any,
    collections::HashMap,
    convert::AsRef,
    sync::{Arc, RwLock},
};
use strum::IntoEnumIterator;
use tracing::trace;

#[derive(Clone)]
pub struct DynDbSetRef<T>(Arc<dyn DbCollection<Item = T>>);

pub struct Store {
    collections: ShareLock<HashMap<StoreIden, Arc<dyn Any + Send + Sync + 'static>>>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Self {
        Self {
            collections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn collection<DATA>(&self) -> Arc<dyn DbCollection<Item = DATA>>
    where
        DATA: DbCollectionIden + Send + Sync + 'static,
    {
        let collections = self.collections.read().unwrap();

        #[allow(clippy::expect_fun_call)]
        let collection = collections.get(&DATA::iden()).expect(&format!(
            "fail to get collection: {}",
            DATA::iden().as_ref()
        ));

        #[allow(clippy::expect_fun_call)]
        collection
            .downcast_ref::<DynDbSetRef<DATA>>()
            .map(|v| v.0.clone())
            .expect(&format!(
                "fail to get collection: {}",
                DATA::iden().as_ref()
            ))
    }

    pub fn register<DATA>(
        &self,
        collection: Arc<dyn DbCollection<Item = DATA> + Send + Sync + 'static>,
    ) where
        DATA: DbCollectionIden + 'static,
    {
        let mut collections = self.collections.write().unwrap();
        collections.insert(DATA::iden(), Arc::new(DynDbSetRef::<DATA>(collection)));
    }

    pub fn tasks(&self) -> Arc<dyn DbCollection<Item = data::Task>> {
        self.collection()
    }

    pub fn procs(&self) -> Arc<dyn DbCollection<Item = data::Proc>> {
        self.collection()
    }

    pub fn packages(&self) -> Arc<dyn DbCollection<Item = data::Package>> {
        self.collection()
    }

    pub fn models(&self) -> Arc<dyn DbCollection<Item = data::Model>> {
        self.collection()
    }

    pub fn messages(&self) -> Arc<dyn DbCollection<Item = data::Message>> {
        self.collection()
    }

    pub fn events(&self) -> Arc<dyn DbCollection<Item = data::Event>> {
        self.collection()
    }

    pub fn publish(&self, pack: &Package) -> Result<bool> {
        trace!("store::publish({})", pack.id);
        if pack.id.is_empty() {
            return Err(ActError::Action("missing id in package".into()));
        }

        let packages = self.packages();
        match packages.find(&pack.id) {
            Ok(m) => {
                let data = Package {
                    create_time: m.create_time,
                    update_time: utils::time::time_millis(),
                    ..pack.clone()
                };
                packages.update(&data)
            }
            Err(_) => {
                let data = Package {
                    create_time: utils::time::time_millis(),
                    ..pack.clone()
                };
                packages.create(&data)
            }
        }
    }
    pub fn deploy(&self, model: &Workflow) -> Result<bool> {
        trace!("store::deploy({})", model.id);
        if model.id.is_empty() {
            return Err(ActError::Model("missing id in model".into()));
        }
        let models = self.models();
        match models.find(&model.id) {
            Ok(m) => {
                let text = serde_yaml::to_string(model).unwrap();
                let data = Model {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    data: text.clone(),
                    ver: m.ver + 1,
                    size: text.len() as i32,
                    create_time: m.create_time,
                    update_time: utils::time::time_millis(),
                    timestamp: utils::time::timestamp(),
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
                    size: text.len() as i32,
                    create_time: utils::time::time_millis(),
                    update_time: 0,
                    timestamp: utils::time::timestamp(),
                };
                models.create(&data)
            }
        }
    }

    pub fn init(&self) {
        let mem = MemStore::new();
        let mut collections = self.collections.write().unwrap();
        for item in StoreIden::iter() {
            // fill the mem store when there is no collection
            collections
                .entry(item.clone())
                .or_insert_with(|| match item {
                    StoreIden::Packages => Arc::new(DynDbSetRef(mem.packages())),
                    StoreIden::Models => Arc::new(DynDbSetRef(mem.models())),
                    StoreIden::Procs => Arc::new(DynDbSetRef(mem.procs())),
                    StoreIden::Tasks => Arc::new(DynDbSetRef(mem.tasks())),
                    StoreIden::Messages => Arc::new(DynDbSetRef(mem.messages())),
                    StoreIden::Events => Arc::new(DynDbSetRef(mem.events())),
                });
        }
    }
}
