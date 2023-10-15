use crate::{
    sch::{Proc, Task},
    store::{self, Store},
    utils::{self, Id},
    Engine, Result, ShareLock, StoreAdapter,
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

    pub fn init(&self, engine: &Engine) {
        #[cfg(feature = "local_store")]
        {
            *self.store.write().unwrap() = Arc::new(Store::local(&engine.options().data_dir));
        }
        if let Some(store) = engine.adapter().store() {
            *self.store.write().unwrap() = Arc::new(Store::create(store));
        }
    }

    pub fn close(&self) {
        self.store.read().unwrap().flush();
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
            vars: utils::vars::to_string(&proc.env().vars())
                .expect("fail to convert vars to string"),
            timestamp: proc.timestamp(),
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
                procs.push(proc.id(), proc.clone());
                on_load(proc);
            }
        }
        Ok(())
    }

    #[instrument]
    pub fn upsert(&self, task: &Task) -> Result<()> {
        let store = self.store.read().unwrap();
        // update proc when updating the task
        let mut proc = store.procs().find(&task.proc_id)?;
        proc.vars = utils::vars::to_string(&task.proc().env().vars())?;
        proc.start_time = task.proc().start_time();
        proc.end_time = task.proc().end_time();
        proc.state = task.proc().state().into();
        store.procs().update(&proc)?;

        let id = Id::new(&task.proc_id, &task.id);
        match store.tasks().find(&id.id()) {
            Ok(mut store_task) => {
                let state = task.state();
                store_task.state = state.into();
                store_task.action_state = task.action_state().into();
                store_task.end_time = task.end_time();
                store_task.vars = utils::vars::to_string(&task.vars())?;
                store.tasks().update(&store_task)?;
            }
            Err(_) => {
                let tid = &task.id;
                let nid = task.node_id();
                let task = store::data::Task {
                    id: id.id(),
                    prev: task.prev(),
                    name: task.node.data().name(),
                    kind: task.node.kind().to_string(),
                    proc_id: task.proc_id.clone(),
                    task_id: tid.clone(),
                    node_id: nid,
                    state: task.state().into(),
                    action_state: task.action_state().into(),
                    start_time: task.start_time(),
                    end_time: task.end_time(),
                    vars: utils::vars::to_string(&task.vars())?,
                    timestamp: task.timestamp,
                };
                store.tasks().create(&task)?;
            }
        }

        Ok(())
    }
}
