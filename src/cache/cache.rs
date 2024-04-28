use crate::{
    engine::Runtime,
    sch::{Proc, Task},
    store::{self, Store},
    utils::Id,
    ActError, Result, ShareLock, StoreAdapter,
};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, RwLock},
};
use tracing::{error, info, instrument};

#[derive(Clone)]
pub struct Cache {
    procs: ShareLock<LruCache<String, Arc<Proc>>>,
    store: ShareLock<Arc<Store>>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("cap", &self.cap())
            .field("count", &self.count())
            .finish()
    }
}

impl Cache {
    pub fn new(cap: usize) -> Self {
        Self {
            procs: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(cap).unwrap()))),
            store: Arc::new(RwLock::new(Store::default())),
        }
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.read().unwrap().clone()
    }

    pub fn cap(&self) -> usize {
        self.procs.read().unwrap().cap().into()
    }

    pub fn count(&self) -> usize {
        self.procs.read().unwrap().len()
    }

    pub fn init(&self) {
        Runtime::with(|runtime| {
            #[cfg(feature = "store")]
            {
                let config = runtime.config();
                *self.store.write().unwrap() =
                    Arc::new(Store::local(&config.data_dir, &config.db_name));
            }
            if let Some(store) = runtime.adapter().store() {
                *self.store.write().unwrap() = Arc::new(Store::create(store));
            }
        });
    }

    pub fn close(&self) {
        self.store.read().unwrap().close();
    }

    pub fn procs(&self) -> Vec<Arc<Proc>> {
        let mut procs = Vec::new();
        for (_, proc) in self.procs.read().unwrap().iter() {
            procs.push(proc.clone());
        }
        procs
    }

    #[instrument]
    pub fn push(&self, proc: &Arc<Proc>) {
        let mut procs = self.procs.write().unwrap();
        procs.push(proc.id(), proc.clone());
        let store = self.store.read().unwrap();
        let model = proc.model();
        let data = store::data::Proc {
            id: proc.id(),
            model: model
                .to_json()
                .expect("fail to convert model to json string"),
            mid: model.id,
            name: model.name,
            state: proc.state().into(),
            start_time: proc.start_time(),
            end_time: proc.end_time(),
            timestamp: proc.timestamp(),
            root_tid: proc.root_tid().unwrap_or_default(),
            env_local: proc.env_local().to_string(),
            err: proc.err().map(|err| err.to_string()),
        };
        store.procs().create(&data).expect("failed to create proc");
    }

    #[instrument]
    pub fn proc(&self, pid: &str) -> Option<Arc<Proc>> {
        let mut procs = self.procs.write().unwrap();
        match procs.get(pid) {
            Some(proc) => Some(proc.clone()),
            None => {
                let store = self.store.read().unwrap();
                if let Some(proc) = store.load_proc(pid).unwrap_or_else(|err| {
                    error!("cache.proc store.loadproc={}", err);
                    None
                }) {
                    info!("load: {:?}", proc);
                    // add to cache
                    procs.push(proc.id(), proc.clone());
                    return Some(proc);
                }

                None
            }
        }
    }

    #[instrument]
    pub fn remove(&self, pid: &str) -> Result<bool> {
        self.procs.write().unwrap().pop(pid);
        self.store.read().unwrap().remove_proc(pid)?;
        Ok(true)
    }

    #[instrument(skip(on_load))]
    pub fn restore<F: Fn(&Arc<Proc>)>(&self, on_load: F) -> Result<()> {
        let store = self.store.read().unwrap();
        let mut procs = self.procs.write().unwrap();
        if procs.len() < procs.cap().get() / 2 {
            let cap = procs.cap().get() - procs.len();
            for ref proc in store.load(cap)? {
                if !procs.contains(&proc.id()) {
                    procs.put(proc.id(), proc.clone());
                    on_load(proc);
                }
            }
        }
        Ok(())
    }

    #[instrument]
    pub fn upsert(&self, task: &Task) -> Result<()> {
        let store = self.store.read().unwrap();
        // update proc when updating the task
        let mut proc = store.procs().find(&task.proc_id)?;
        proc.start_time = task.proc().start_time();
        proc.end_time = task.proc().end_time();
        proc.state = task.proc().state().into();
        store.procs().update(&proc)?;

        let id = Id::new(&task.proc_id, &task.id);
        match store.tasks().find(&id.id()) {
            Ok(mut store_task) => {
                let state = task.state();
                store_task.state = state.into();
                store_task.end_time = task.end_time();
                store_task.hooks = serde_json::to_string(&task.hooks())
                    .map_err(|err| ActError::Store(err.to_string()))?;
                store_task.data = task.data().to_string();
                store_task.err = task.err().map(|err| err.to_string());
                store.tasks().update(&store_task)?;
            }
            Err(_) => {
                let tid = &task.id;
                let task = store::data::Task {
                    id: id.id(),
                    prev: task.prev(),
                    name: task.node.content.name(),
                    kind: task.node.r#type(),
                    proc_id: task.proc_id.clone(),
                    task_id: tid.clone(),
                    node_id: task.node.id().to_string(),
                    state: task.state().into(),
                    data: task.data().to_string(),
                    start_time: task.start_time(),
                    end_time: task.end_time(),
                    hooks: serde_json::to_string(&task.hooks())
                        .map_err(|err| ActError::Store(err.to_string()))?,
                    timestamp: task.timestamp,
                    err: task.err().map(|err| err.to_string()),
                };
                store.tasks().create(&task)?;
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn uncache(&self, pid: &str) {
        self.procs.write().unwrap().pop(pid);
    }
}
