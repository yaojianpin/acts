use crate::{
    Engine, Result,
    scheduler::{Process, Runtime, Task},
    store::Store,
};
use moka::sync::Cache as MokaCache;
use std::sync::Arc;
use tracing::{debug, error, instrument};

#[derive(Clone)]
pub struct Cache {
    cap: usize,
    procs: MokaCache<String, Arc<Process>>,
    store: Arc<Store>,
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
            store: Arc::new(Store::new()),
        }
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn count(&self) -> usize {
        self.procs.run_pending_tasks();
        self.procs.entry_count() as usize
    }

    pub fn init(&self, _engine: &Engine) {
        debug!("cache::init");
        self.store.init();
    }

    pub fn close(&self) {}

    #[instrument]
    pub fn push_proc(&self, proc: &Arc<Process>) {
        self.push_proc_pri(proc, true);
    }

    pub fn procs(&self) -> Vec<Arc<Process>> {
        let mut procs = Vec::new();
        for (_, proc) in self.procs.iter() {
            procs.push(proc.clone());
        }
        procs
    }

    #[instrument]
    pub fn proc(&self, pid: &str, rt: &Arc<Runtime>) -> Option<Arc<Process>> {
        debug!("process: pid={pid}");
        match self.get_proc(pid) {
            Some(proc) => Some(proc.clone()),
            None => {
                if let Some(proc) = self.store.load_proc(pid, rt).unwrap_or_else(|err| {
                    error!("cache.process store.loadproc={}", err);
                    eprintln!("cache.process store.loadproc={}", err);
                    None
                }) {
                    debug!("loaded: {:?}", proc);
                    debug!("tasks: {:?}", proc.tasks());
                    // add to cache
                    self.push_proc_pri(&proc, false);
                    return Some(proc);
                }
                None
            }
        }
    }

    #[instrument]
    pub fn remove(&self, pid: &str) -> Result<bool> {
        debug!("remove pid={pid}");
        self.procs.remove(pid);
        self.store.remove_proc(pid)?;
        Ok(true)
    }

    #[instrument(skip(on_load))]
    pub fn restore<F: Fn(&Arc<Process>)>(&self, rt: &Arc<Runtime>, on_load: F) -> Result<()> {
        debug!("restore");
        let cap = self.cap();
        let count = self.count();
        let mut check_point = cap / 2;
        if check_point == 0 {
            check_point = cap;
        }
        if count < check_point {
            let cap = cap - count;
            for ref proc in self.store.load(cap, rt)? {
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

    fn get_proc(&self, pid: &str) -> Option<Arc<Process>> {
        self.procs.get(pid)
    }

    pub(super) fn push_proc_pri(&self, proc: &Arc<Process>, save: bool) {
        debug!("push process pid={}", proc.id());
        if save {
            self.store
                .upsert_proc(proc)
                .expect("fail to upsert process");
        }
        self.procs.insert(proc.id().to_string(), proc.clone());
    }

    pub(super) fn push_task_pri(&self, task: &Arc<Task>, save: bool) -> Result<()> {
        let p = task.proc();
        if save {
            // update process when updating the task
            let collection = self.store.procs();
            let mut proc = collection.find(&task.pid)?;
            proc.end_time = p.end_time();
            proc.state = p.state().into();

            collection.update(&proc)?;
            self.store.upsert_task(task)?;
        }

        if let Some(proc) = self.procs.get(&task.pid) {
            proc.set_pure_state(p.state());
            proc.set_end_time(p.end_time());
            proc.push_task(task.clone());
        }

        Ok(())
    }
}
