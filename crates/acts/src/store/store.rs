use std::sync::Arc;
use std::sync::Mutex;

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use tracing::trace;

use crate::store::KvStore;
use crate::{
    ActError, Config, Result, Workflow,
    store::{Model, Package},
    utils,
};

use super::memory::MemoryStore;
use super::{DbCollection, DbCollectionIden, collection::KvCollection, data};

pub struct Store {
    kv: Arc<dyn KvStore>,
    overrides: Arc<Mutex<Option<Arc<dyn KvStore>>>>,
}

impl Store {
    pub fn new(kv: Arc<dyn KvStore>) -> Self {
        Self {
            kv,
            overrides: Arc::new(Mutex::new(None)),
        }
    }

    pub fn create(config: &Config) -> crate::Result<Self> {
        #[allow(unused_mut)]
        let mut kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());

        #[allow(unused_variables, unused_assignments)]
        if let Some(db) = &config.data.db {
            #[cfg(feature = "store-sqlite")]
            {
                kv = Arc::new(super::SqliteStore::open(&db.database_url)?);
            }
            #[cfg(feature = "store-postgres")]
            {
                kv = Arc::new(super::PostgresStore::open(&db.database_url)?);
            }
            #[cfg(feature = "store-redis")]
            {
                kv = Arc::new(super::RedisStore::open(&db.database_url)?);
            }
            #[cfg(feature = "store-nats")]
            {
                kv = Arc::new(super::NatsStore::open(&db.database_url)?);
            }
            #[cfg(feature = "store-sled")]
            {
                kv = Arc::new(super::SledStore::open(&db.database_url)?);
            }
        }

        Ok(Self {
            kv,
            overrides: Arc::new(Mutex::new(None)),
        })
    }

    pub fn register(&self, kv: Arc<dyn KvStore + Send + Sync + 'static>) {
        *self.overrides.lock().unwrap() = Some(kv);
    }

    fn collection<DATA>(&self) -> Arc<dyn DbCollection<Item = DATA>>
    where
        DATA:
            DbCollectionIden + Serialize + DeserializeOwned + Send + Sync + Clone + Debug + 'static,
    {
        let prefix = DATA::iden().as_ref().to_string();
        if let Some(kv) = self.overrides.lock().unwrap().as_ref() {
            return Arc::new(KvCollection::new(&prefix, kv.clone()));
        }

        Arc::new(KvCollection::new(&prefix, self.kv.clone()))
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
                    desc: model.desc.clone(),
                    data: text.clone(),
                    ver: m.ver.clone(),
                    size: text.len() as i32,
                    create_time: m.create_time,
                    update_time: utils::time::time_millis(),
                    timestamp: utils::time::timestamp(),
                    v: Model::version(),
                };
                models.update(&data)
            }
            Err(_) => {
                let text = serde_yaml::to_string(model).unwrap();
                let data = Model {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    desc: model.desc.clone(),
                    data: text.clone(),
                    ver: "0.1.0".to_string(),
                    size: text.len() as i32,
                    create_time: utils::time::time_millis(),
                    update_time: 0,
                    timestamp: utils::time::timestamp(),
                    v: Model::version(),
                };
                models.create(&data)
            }
        }
    }
}
