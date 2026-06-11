use crate::{
    Config, Result,
    scheduler::{Process, Runtime, Task},
    store::Store,
};
use moka::sync::Cache as MokaCache;
use std::sync::Arc;
use tracing::{debug, instrument};

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
    pub fn new(config: &Config) -> crate::Result<Self> {
        Ok(Self {
            cap: config.cache_cap() as usize,
            procs: MokaCache::new(config.cache_cap() as u64),
            store: Arc::new(Store::create(config)?),
        })
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

    pub fn close(&self) {}

    #[instrument]
    pub fn push_proc(&self, proc: &Arc<Process>) -> Result<()> {
        self.push_proc_pri(proc, true)?;

        Ok(())
    }

    pub fn procs(&self) -> Vec<Arc<Process>> {
        let mut procs = Vec::new();
        for (_, proc) in self.procs.iter() {
            procs.push(proc.clone());
        }
        procs
    }

    #[instrument]
    pub fn proc(&self, pid: &str, rt: &Arc<Runtime>) -> Result<Option<Arc<Process>>> {
        debug!("process: pid={pid}");
        match self.get_proc(pid) {
            Some(proc) => Ok(Some(proc.clone())),
            None => {
                if let Some(proc) = self.store.load_proc(pid, rt)? {
                    debug!("loaded: {:?}", proc);
                    debug!("tasks: {:?}", proc.tasks());
                    // add to cache
                    self.push_proc_pri(&proc, false)?;
                    return Ok(Some(proc));
                }
                Ok(None)
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

    #[instrument(skip())]
    pub fn restore(&self, rt: &Arc<Runtime>) -> Result<()> {
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
                    self.push_proc_pri(proc, false)?;
                    if proc.state().is_none() {
                        proc.start()?;
                    }
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

    pub(super) fn push_proc_pri(&self, proc: &Arc<Process>, save: bool) -> Result<()> {
        debug!("push process pid={}", proc.id());
        if save {
            self.store.upsert_proc(proc)?;
        }
        self.procs.insert(proc.id().to_string(), proc.clone());

        Ok(())
    }

    pub(super) fn push_task_pri(&self, task: &Arc<Task>, save: bool) -> Result<()> {
        let p = task.proc();
        if save {
            self.store.upsert_task(task)?;
            // update root task data when updating task
            if let Some(root) = p.root() {
                self.store.upsert_task(&root)?;
            }
            // update process to store when process state is completed
            if p.state().is_completed() {
                let collection = self.store.procs();
                let mut proc = collection.find(&task.pid)?;
                proc.end_time = p.end_time();
                proc.state = p.state().into();
                collection.update(&proc)?;
            }
        }

        if let Some(proc) = self.procs.get(&task.pid) {
            proc.set_pure_state(p.state());
            proc.set_end_time(p.end_time());
            proc.push_task(task.clone());
        }

        Ok(())
    }
}
