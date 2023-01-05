use crate::{
    debug,
    sch::{
        event::{EventAction, EventData},
        ActId, Message, Proc, Scheduler, Task,
    },
    store::Store,
    utils::{self},
    Engine, ShareLock,
};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, RwLock},
};

#[derive(Clone)]
pub struct Cache {
    procs: ShareLock<LruCache<String, Proc>>,
    store: Arc<Store>,
    scher: ShareLock<Option<Arc<Scheduler>>>,
}

impl Cache {
    pub fn new(cap: usize) -> Self {
        Self {
            procs: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(cap).unwrap()))),
            store: Arc::new(Store::new()),
            scher: Arc::new(RwLock::new(None)),
        }
    }

    pub fn init(&self, engine: &Engine) {
        debug!("cache::init");
        let scher = engine.scher();
        *self.scher.write().unwrap() = Some(scher.clone());

        self.store.init(engine);

        {
            let cache = self.clone();
            let s = scher.clone();
            scher.on_proc(move |proc: &Proc, data: &EventData| {
                debug!("sch::cache::on_proc: {}", data);
                if data.action == EventAction::Next {
                    let pid = data.pid.clone();
                    cache.remove(&pid);
                    cache.restore(s.clone());
                } else {
                    cache.store.update_proc(proc);
                }
            });
        }
        {
            let cache = self.clone();
            scher.on_task(move |task: &Task, data: &EventData| {
                debug!("sch::cache::on_task: tid={}, data={}", task.tid(), data);
                if data.action == EventAction::Create {
                    cache.store.create_task(task, &task.tid());
                } else {
                    cache.store.update_task(task, &task.tid(), &data.vars);
                }
            });
        }
        {
            let cache = self.clone();
            scher.on_message(move |msg: &Message| {
                debug!("sch::cache::on_message: {}", msg);
                cache.store.create_message(msg)
            });
        }
    }

    pub fn close(&self) {
        self.store.flush();
    }

    pub fn push(&self, proc: &Proc) {
        debug!("sch::cache::push({})", proc.pid());
        self.store.create_proc(proc);
        self.procs.write().unwrap().push(proc.pid(), proc.clone());
    }

    pub fn proc(&self, pid: &str) -> Option<Proc> {
        let mut procs = self.procs.write().unwrap();
        match procs.get(pid) {
            Some(proc) => Some(proc.clone()),
            None => {
                if let Some(scher) = &*self.scher.read().unwrap() {
                    return self.store.proc(pid, scher);
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

    fn remove(&self, pid: &str) {
        debug!("sch::cache::remove pid={}", pid);
        self.procs.write().unwrap().pop(pid);
        self.store.remove_proc(&pid);
    }

    fn restore(&self, scher: Arc<Scheduler>) {
        debug!("sch::cache::restore");
        let mut procs = self.procs.write().unwrap();
        if procs.len() < procs.cap().get() / 2 {
            let cap = procs.cap().get() - procs.len();
            for ref proc in self.store.load(scher, cap) {
                procs.push(proc.pid(), proc.clone());
                self.send(proc);
            }
        }
    }

    // fn take(&self) {
    //     let cap = self.cap();
    //     debug!("sch::cache::take: cap={}", cap);
    //     let procs = self.procs.read().unwrap();
    //     let iter = procs.iter().take(cap);
    //     for (_, proc) in iter {
    //         self.send(proc);
    //     }
    // }

    // fn cap(&self) -> usize {
    //     let procs = self.procs.read().unwrap();
    //     procs.cap().get() - procs.len()
    // }

    fn send(&self, proc: &Proc) {
        if let Some(scher) = &*self.scher.read().unwrap() {
            scher.sched_proc(proc);
        }
    }
}
