use crate::{
    sch::{Proc, Runtime, Task},
    store::Store,
    Engine, Result, ShareLock, StoreAdapter,
};
use moka::sync::Cache as MokaCache;
use std::sync::{Arc, RwLock};
use tracing::{debug, error, instrument};

#[derive(Clone)]
pub struct Cache {
    cap: usize,
    procs: MokaCache<String, Arc<Proc>>,
    store: ShareLock<Arc<Store>>,
    // sync: Arc<Mutex<usize>>,
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
            cap,
            procs: MokaCache::new(cap as u64),
            store: Arc::new(RwLock::new(Store::default())),
            // sync: Arc::new(Mutex::new(0)),
        }
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.read().unwrap().clone()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn count(&self) -> usize {
        self.procs.run_pending_tasks();
        self.procs.entry_count() as usize
    }

    pub fn init(&self, engine: &Engine) {
        debug!("cache::init");
        #[cfg(feature = "store")]
        {
            let config = engine.config();
            *self.store.write().unwrap() =
                Arc::new(Store::local(&config.data_dir, &config.db_name));
        }
        if let Some(store) = engine.adapter().store() {
            *self.store.write().unwrap() = Arc::new(Store::create(store));
        }
    }

    pub fn close(&self) {
        // let _ = self.sync.lock();
        self.store.read().unwrap().close();
    }

    #[instrument]
    pub fn push_proc(&self, proc: &Arc<Proc>) {
        self.push_proc_pri(proc, true);
    }

    pub fn procs(&self) -> Vec<Arc<Proc>> {
        let mut procs = Vec::new();
        for (_, proc) in self.procs.iter() {
            procs.push(proc.clone());
        }
        procs
    }

    #[instrument]
    pub fn proc(&self, pid: &str, rt: &Arc<Runtime>) -> Option<Arc<Proc>> {
        // let _lock = self.sync.lock().unwrap();
        debug!("proc: pid={pid}");
        match self.get_proc(pid) {
            Some(proc) => Some(proc.clone()),
            None => {
                let store = self.store.read().unwrap();
                if let Some(proc) = store.load_proc(pid, rt).unwrap_or_else(|err| {
                    error!("cache.proc store.loadproc={}", err);
                    eprintln!("cache.proc store.loadproc={}", err);
                    None
                }) {
                    debug!("loaded: {:?}", proc);
                    debug!("tasks: {:?}", proc.tasks());
                    // add to cache
                    self.push_proc_pri(&proc, false);
                    return Some(proc);
                }
                error!("not to load proc:{}", pid);
                None
            }
        }
    }

    #[instrument]
    pub fn remove(&self, pid: &str) -> Result<bool> {
        // let _lock = self.sync.lock().unwrap();
        debug!("remove pid={pid}");
        self.procs.remove(pid);
        self.store.read().unwrap().remove_proc(pid)?;
        Ok(true)
    }

    #[instrument(skip(on_load))]
    pub fn restore<F: Fn(&Arc<Proc>)>(&self, rt: &Arc<Runtime>, on_load: F) -> Result<()> {
        debug!("restore");
        let store = self.store.read().unwrap();
        let cap = self.cap();
        let count = self.count();
        let mut check_point = cap / 2;
        if check_point == 0 {
            check_point = cap;
        }
        if count < check_point {
            let cap = cap - count;
            for ref proc in store.load(cap, rt)? {
                if !self.procs.contains_key(proc.id()) {
                    self.push_proc_pri(proc, false);
                    on_load(proc);
                }
            }
        }
        Ok(())
    }

    #[instrument]
    pub fn upsert(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_pri(task, true)
    }

    #[cfg(test)]
    pub fn uncache(&self, pid: &str) {
        self.procs.remove(pid);
    }

    fn get_proc(&self, pid: &str) -> Option<Arc<Proc>> {
        self.procs.get(pid)
    }

    pub(super) fn push_proc_pri(&self, proc: &Arc<Proc>, save: bool) {
        debug!("push proc pid={}", proc.id());
        if save {
            let store = self.store.read().unwrap();
            store.upsert_proc(proc).expect("fail to upsert proc");
        }
        self.procs.insert(proc.id().to_string(), proc.clone());
    }

    pub(super) fn push_task_pri(&self, task: &Arc<Task>, save: bool) -> Result<()> {
        let p = task.proc();
        if save {
            let store = self.store.read().unwrap();
            // update proc when updating the task
            let mut proc = store.procs().find(&task.pid)?;
            proc.end_time = p.end_time();
            proc.state = p.state().into();
            store.procs().update(&proc)?;

            store.upsert_task(task)?;
        }

        if let Some(proc) = self.procs.get(&task.pid) {
            proc.set_pure_state(p.state());
            proc.set_end_time(p.end_time());
            proc.push_task(task.clone());
        }

        Ok(())
    }
}
