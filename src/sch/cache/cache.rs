use crate::{
    debug,
    sch::{
        cache::StoreMediator,
        event::{EventAction, EventData},
        Message, Proc, Scheduler, Task,
    },
    utils::{self},
    ActResult, Engine, ShareLock,
};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, RwLock},
};

#[derive(Clone)]
pub struct Cache {
    procs: ShareLock<LruCache<String, Proc>>,
    scher: ShareLock<Option<Arc<Scheduler>>>,
    store: ShareLock<Option<StoreMediator>>,
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
        let scher = engine.scher();
        *self.scher.write().unwrap() = Some(scher.clone());
        *self.store.write().unwrap() = Some(StoreMediator::new(engine.store()));

        {
            let cache = self.clone();

            let s = scher.clone();

            let emitter = engine.emitter();
            emitter.on_proc(move |proc: &Proc, data: &EventData| {
                debug!("sch::cache::on_proc: {}", data);
                if data.action == EventAction::Next {
                    let pid = data.pid.clone();
                    cache
                        .remove(&pid)
                        .expect(&format!("fail to remove pid={}", pid));
                    cache.restore(s.clone());
                } else {
                    if let Some(store) = &*cache.store.read().unwrap() {
                        store.update_proc(proc);
                    }
                }
            });
        }
        {
            let cache = self.clone();
            let emitter = engine.emitter();
            emitter.on_task(move |task: &Task, data: &EventData| {
                debug!("sch::cache::on_task: tid={}, data={}", task.tid, data);
                if let Some(store) = &*cache.store.read().unwrap() {
                    if data.action == EventAction::Create {
                        store.create_task(task);
                    } else {
                        store.update_task(task, &data.vars);
                    }
                }
            });
        }
        {
            let cache = self.clone();
            let emitter = engine.emitter();
            emitter.on_message(move |msg: &Message| {
                debug!("sch::cache::on_message: {:?}", msg);
                if let Some(store) = &*cache.store.read().unwrap() {
                    store.create_message(msg)
                }
            });
        }
    }

    pub fn close(&self) {
        if let Some(store) = &*self.store.read().unwrap() {
            store.flush();
        }
    }

    pub fn push(&self, proc: &Proc) {
        debug!("sch::cache::push({})", proc.pid());
        self.procs.write().unwrap().push(proc.pid(), proc.clone());
        if let Some(store) = &*self.store.read().unwrap() {
            store.create_proc(proc);
        }
    }

    pub fn proc(&self, pid: &str) -> Option<Proc> {
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
            return proc.message(&id.tid());
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
            store.remove_proc(&pid)?;
        }

        Ok(true)
    }

    fn restore(&self, scher: Arc<Scheduler>) {
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

    fn send(&self, proc: &Proc) {
        if let Some(scher) = &*self.scher.read().unwrap() {
            scher.sched_proc(proc);
        }
    }
}
