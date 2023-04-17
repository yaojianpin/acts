use crate::{
    event::{EventAction, EventData, Message},
    sch::{Proc, Scheduler, Task},
    store::{self, Store},
    utils::{self, Id},
    ActResult, Engine, ShareLock, StoreAdapter,
};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, RwLock},
};
use tracing::debug;

#[derive(Clone)]
pub struct Cache {
    procs: ShareLock<LruCache<String, Arc<Proc>>>,
    scher: ShareLock<Option<Arc<Scheduler>>>,
    store: ShareLock<Option<Arc<Store>>>,
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
        debug!("cache::init");

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

    pub fn create_proc(&self, proc: &Arc<Proc>) {
        debug!("sch::cache::create_proc({})", proc.pid());
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
                vars: utils::vars::to_string(&proc.vm().vars()),
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

    pub fn message(&self, id: &str) -> Option<Message> {
        let id = utils::Id::from(id);
        if let Some(proc) = self.proc(&id.pid()) {
            return proc.message(&id.id());
        }
        None
    }

    pub fn message_by_uid(&self, pid: &str, uid: &str) -> Option<Message> {
        if let Some(proc) = self.proc(pid) {
            return proc.message_by_uid(uid);
        }
        None
    }

    // pub fn nearest_done_task_by_uid(&self, pid: &str, uid: &str) -> Option<Arc<Task>> {
    //     if let Some(proc) = self.proc(pid) {
    //         let mut tasks = proc.task_by_uid(uid, TaskState::Success);
    //         if tasks.len() > 0 {
    //             tasks.sort_by(|a, b| a.end_time().cmp(&b.end_time()));
    //             let task = tasks.get(0).unwrap().clone();
    //             return Some(task);
    //         }
    //     }
    //     None
    // }

    pub fn remove(&self, pid: &str) -> ActResult<bool> {
        debug!("sch::cache::remove pid={}", pid);
        self.procs.write().unwrap().pop(pid);
        if let Some(store) = &*self.store.read().unwrap() {
            store.procs().delete(&pid)?;
        }

        Ok(true)
    }

    pub fn restore(&self, scher: Arc<Scheduler>) {
        debug!("sch::cache::restore");
        if let Some(store) = &*self.store.read().unwrap() {
            let mut procs = self.procs.write().unwrap();
            if procs.len() < procs.cap().get() / 2 {
                let cap = procs.cap().get() - procs.len();
                for ref proc in store.load(scher, cap) {
                    procs.push(proc.pid(), proc.clone());
                    self.send(proc);
                }
            }
        }
    }

    pub fn upsert_task(&self, task: &Task, data: &EventData) {
        if let Some(store) = &*self.store.read().unwrap() {
            if data.action == EventAction::Create {
                let tid = &task.tid;
                let nid = task.nid();
                let id = Id::new(&task.pid, tid);
                let task = store::Task {
                    id: id.id(),
                    kind: task.node.kind().to_string(),
                    pid: task.pid.clone(),
                    tid: tid.clone(),
                    nid: nid,
                    state: task.state().into(),
                    start_time: task.start_time(),
                    end_time: task.end_time(),
                    uid: match task.uid() {
                        Some(u) => u,
                        None => "".to_string(),
                    },
                };
                store.tasks().create(&task).expect("failed to create task");
            } else {
                let pid = &task.pid;

                let mut proc = store.procs().find(pid).expect("get store proc");
                proc.vars = utils::vars::to_string(&data.vars);
                store.procs().update(&proc).expect("update store proc vars");

                let id = Id::new(pid, &task.tid);
                let state = task.state();
                let mut task = store.tasks().find(&id.id()).expect("get store task");
                task.state = state.into();
                store.tasks().update(&task).expect("failed to update task");
            }
        }
    }

    pub fn create_message(&self, msg: &Message) {
        let uid = match &msg.uid {
            Some(uid) => uid,
            None => "",
        };
        if let Some(store) = &*self.store.read().unwrap() {
            store
                .messages()
                .create(&store::Message {
                    id: msg.id.clone(),
                    pid: msg.pid.clone(),
                    tid: msg.tid.clone(),
                    uid: uid.to_string(),
                    vars: utils::vars::to_string(&msg.vars),
                    create_time: msg.create_time,
                    update_time: msg.update_time,
                    state: msg.state.clone().into(),
                })
                .expect("create proc message");
        }
    }

    fn send(&self, proc: &Arc<Proc>) {
        if let Some(scher) = &*self.scher.read().unwrap() {
            scher.sched_proc(proc);
        }
    }
}
