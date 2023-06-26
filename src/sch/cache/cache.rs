use crate::{
    event::EventData,
    sch::{Act, Proc, Scheduler, Task},
    store::{self, Store},
    utils::{self, Id},
    ActResult, Engine, ShareLock, StoreAdapter,
};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, RwLock},
};
use tracing::instrument;

#[derive(Clone)]
pub struct Cache {
    procs: ShareLock<LruCache<String, Arc<Proc>>>,
    scher: ShareLock<Option<Arc<Scheduler>>>,
    store: ShareLock<Option<Arc<Store>>>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").finish()
    }
}

impl Cache {
    pub fn new(cap: usize) -> Self {
        Self {
            procs: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(cap).unwrap()))),
            scher: Arc::new(RwLock::new(None)),
            store: Arc::new(RwLock::new(None)),
        }
    }

    pub fn init(&self, engine: &Engine) {
        // init store from adapter
        *self.store.write().unwrap() = Some(engine.store());

        let scher = engine.scher();
        *self.scher.write().unwrap() = Some(scher.clone());
    }

    pub fn close(&self) {
        if let Some(store) = &*self.store.read().unwrap() {
            store.flush();
        }
    }

    pub fn count(&self) -> usize {
        self.procs.read().unwrap().len()
    }

    #[instrument]
    pub fn push_proc(&self, proc: &Arc<Proc>) {
        self.procs.write().unwrap().push(proc.pid(), proc.clone());
        if let Some(store) = &*self.store.read().unwrap() {
            let workflow = &*proc.workflow();
            let data = store::Proc {
                id: proc.pid(), // pid is global unique id
                pid: proc.pid(),
                model: serde_yaml::to_string(workflow).unwrap(),
                state: proc.state().into(),
                start_time: proc.start_time(),
                end_time: proc.end_time(),
                vars: utils::vars::to_string(&proc.env.vars()),
            };
            store.procs().create(&data).expect("failed to create proc");
        }
    }

    pub fn proc(&self, pid: &str) -> Option<Arc<Proc>> {
        let mut procs = self.procs.write().unwrap();
        match procs.get(pid) {
            Some(proc) => Some(proc.clone()),
            None => {
                if let Some(scher) = &*self.scher.read().unwrap() {
                    if let Some(store) = &*self.store.read().unwrap() {
                        return store.load_proc(pid, scher);
                    }
                }

                None
            }
        }
    }

    pub fn act(&self, pid: &str, aid: &str) -> Option<Arc<Act>> {
        if let Some(proc) = self.proc(pid) {
            return proc.act(aid);
        }
        None
    }

    #[instrument]
    pub fn remove(&self, pid: &str) -> ActResult<bool> {
        self.procs.write().unwrap().pop(pid);
        if let Some(store) = &*self.store.read().unwrap() {
            store.remove_proc(pid)?;
        }

        Ok(true)
    }

    #[instrument]
    pub fn restore(&self) {
        if let Some(store) = &*self.store.read().unwrap() {
            let mut procs = self.procs.write().unwrap();
            if procs.len() < procs.cap().get() / 2 {
                let cap = procs.cap().get() - procs.len();
                for ref proc in store.load(cap) {
                    procs.push(proc.pid(), proc.clone());
                    self.send(proc);
                }
            }
        }
    }

    #[instrument]
    pub fn upsert_task(&self, task: &Task, data: &EventData) {
        if let Some(store) = &*self.store.read().unwrap() {
            let id = Id::new(&task.pid, &task.tid);
            match store.tasks().find(&id.id()) {
                Ok(mut store_task) => {
                    let pid = &task.pid;

                    let mut proc = store.procs().find(pid).expect("get store proc");
                    proc.vars = utils::vars::to_string(&task.proc.env.vars());
                    store.procs().update(&proc).expect("update store proc vars");

                    let state = task.state();
                    store_task.state = state.into();
                    store_task.end_time = task.end_time();
                    store
                        .tasks()
                        .update(&store_task)
                        .expect("failed to update task");
                }
                Err(_) => {
                    let tid = &task.tid;
                    let nid = task.nid();
                    let task = store::Task {
                        id: id.id(),
                        kind: task.node.kind().to_string(),
                        pid: task.pid.clone(),
                        tid: tid.clone(),
                        nid: nid,
                        state: task.state().into(),
                        start_time: task.start_time(),
                        end_time: task.end_time(),
                    };
                    store.tasks().create(&task).expect("failed to create task");
                }
            }
        }
    }

    #[instrument]
    pub fn upsert_act(&self, act: &Act, data: &EventData) {
        if let Some(store) = &*self.store.read().unwrap() {
            match store.acts().find(&act.id) {
                Ok(mut store_act) => {
                    let mut proc = store.procs().find(&act.pid).expect("get store proc");
                    proc.vars = utils::vars::to_string(&act.task.proc.env.vars());
                    store.procs().update(&proc).expect("update store proc vars");

                    store_act.state = act.state().into();
                    store_act.end_time = act.end_time();
                    store_act.active = act.active();
                    store_act.vars = utils::vars::to_string(&act.vars);
                    store
                        .acts()
                        .update(&store_act)
                        .expect("failed to update act");
                }
                Err(_) => {
                    let store_act = store::Act {
                        id: act.id.clone(),
                        kind: act.kind.to_string(),
                        pid: act.pid.clone(),
                        tid: act.tid.clone(),
                        state: act.state().into(),
                        start_time: act.start_time(),
                        end_time: act.end_time(),
                        vars: utils::vars::to_string(&act.vars),
                        event: data.event.to_string(),
                        active: act.active(),
                    };
                    store
                        .acts()
                        .create(&store_act)
                        .expect("failed to create act");
                }
            }
        }
    }

    fn send(&self, proc: &Arc<Proc>) {
        if let Some(scher) = &*self.scher.read().unwrap() {
            scher.sched_proc(proc);
        }
    }
}
