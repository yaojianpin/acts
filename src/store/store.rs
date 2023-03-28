use crate::{
    adapter::StoreAdapter,
    debug,
    store::{none::NoneStore, Message, Model, Proc, Query, Task},
    utils::{self, Id},
    ActError, ActResult, Engine, ShareLock, Workflow,
};
use std::sync::{Arc, Mutex, RwLock};

#[cfg(feature = "sqlite")]
use crate::store::db::SqliteStore;

#[cfg(feature = "store")]
use crate::store::db::LocalStore;

const LIMIT: usize = 10000;

#[derive(Clone, Debug, PartialEq)]
pub enum StoreKind {
    None,
    #[cfg(feature = "store")]
    Local,
    #[cfg(feature = "sqlite")]
    Sqlite,
    Extern,
}

pub struct Store {
    kind: Arc<Mutex<StoreKind>>,
    base: ShareLock<Arc<dyn StoreAdapter>>,
}

impl Store {
    pub fn new() -> Self {
        #[allow(unused_assignments)]
        let mut store: Arc<dyn StoreAdapter> = Arc::new(NoneStore::new());
        #[allow(unused_assignments)]
        let mut kind = StoreKind::None;

        #[cfg(feature = "store")]
        {
            debug!("store::new");
            store = Arc::new(LocalStore::new());
            kind = StoreKind::Local;
        }

        #[cfg(feature = "sqlite")]
        {
            debug!("sqlite::new");
            store = Arc::new(SqliteStore::new());
            kind = StoreKind::Sqlite;
        }

        Self {
            kind: Arc::new(Mutex::new(kind)),
            base: Arc::new(RwLock::new(store)),
        }
    }

    pub fn init(&self, engine: &Engine) {
        debug!("store::init");
        if let Some(store) = engine.adapter().store() {
            *self.kind.lock().unwrap() = StoreKind::Extern;
            *self.base.write().unwrap() = store;
        }
    }

    pub fn flush(&self) {
        debug!("store::flush");
        self.base.read().unwrap().flush();
    }

    pub fn procs(&self, cap: usize) -> ActResult<Vec<Proc>> {
        debug!("store::procs cap={}", cap);
        let procs = self.base().procs();
        let query = Query::new().set_limit(cap);
        procs.query(&query)
    }

    pub fn proc(&self, pid: &str) -> ActResult<Proc> {
        debug!("store::proc pid={}", pid);
        let procs = self.base().procs();
        procs.find(&pid)
    }

    pub fn models(&self, cap: usize) -> ActResult<Vec<Model>> {
        debug!("store::models({})", cap);
        let query = Query::new().set_limit(cap);
        let models = self.base().models();
        models.query(&query)
    }

    pub fn model(&self, id: &str) -> ActResult<Model> {
        debug!("store::create_model({})", id);
        let models = self.base().models();
        models.find(&id)
    }

    pub fn deploy(&self, model: &Workflow) -> ActResult<bool> {
        debug!("store::create_model({})", model.id);
        if model.id.is_empty() {
            return Err(ActError::OperateError("missing id in model".into()));
        }
        let models = self.base().models();

        match models.find(&model.id) {
            Ok(m) => {
                let text = serde_yaml::to_string(model).unwrap();
                let data = Model {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    model: text.clone(),
                    ver: m.ver + 1,
                    size: text.len() as u32,
                    time: utils::time::time(),
                };
                models.update(&data)
            }
            Err(_) => {
                let text = serde_yaml::to_string(model).unwrap();
                let data = Model {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    model: text.clone(),
                    ver: 1,
                    size: text.len() as u32,
                    time: utils::time::time(),
                };
                models.create(&data)
            }
        }
    }

    pub fn remove_model(&self, id: &str) -> ActResult<bool> {
        debug!("store::remove_model({})", id);
        let base = self.base();
        let models = base.models();
        models.delete(id)
    }

    pub fn create_proc(&self, proc: &Proc) -> ActResult<bool> {
        debug!("store::create_proc({})", proc.pid);
        self.base().procs().create(proc)
    }

    pub fn update_proc(&self, proc: &Proc) -> ActResult<bool> {
        debug!("store::update_proc({})", proc.pid);
        let procs = self.base().procs();
        procs.update(&proc)
    }

    pub fn remove_proc(&self, pid: &str) -> ActResult<bool> {
        debug!("store::remove_proc({})", pid);
        let base = self.base();
        let procs = base.procs();
        let tasks = base.tasks();
        let messages = base.messages();

        let query = Query::new().set_limit(LIMIT).push("pid", pid.into());
        for msg in messages.query(&query)? {
            messages.delete(&msg.id)?;
        }

        for task in tasks.query(&query)? {
            tasks.delete(&task.id)?;
        }
        procs.delete(pid)
    }

    pub fn create_task(&self, task: &Task) -> ActResult<bool> {
        debug!("store::create_task({:?})", task);
        let tasks = self.base().tasks();
        tasks.create(&task)
    }

    pub fn update_task(&self, task: &Task) -> ActResult<bool> {
        debug!("store::update_task({})", task.tid);
        let tasks = self.base().tasks();
        tasks.update(task)
    }

    pub fn remove_task(&self, id: &str) -> ActResult<bool> {
        debug!("store::remove_task({})", id);
        let tasks = self.base().tasks();
        tasks.delete(id)
    }

    pub fn tasks(&self, pid: &str) -> ActResult<Vec<Task>> {
        debug!("store::tasks  pid={}", pid);
        let tasks = self.base().tasks();
        let query = Query::new().set_limit(LIMIT).push("pid", pid.into());
        tasks.query(&query)
    }

    pub fn task(&self, pid: &str, tid: &str) -> ActResult<Task> {
        debug!("store::task pid={}  tid={}", pid, tid);
        let tasks = self.base().tasks();
        let id = Id::new(pid, &tid);
        tasks.find(&id.id())
    }

    pub fn messages(&self, pid: &str) -> ActResult<Vec<Message>> {
        debug!("store::messages  pid={}", pid);
        let messages = self.base().messages();
        let query = Query::new().set_limit(LIMIT).push("pid", pid.into());
        messages.query(&query)
    }

    pub fn message(&self, id: &str) -> ActResult<Message> {
        debug!("store::message id={} ", id);
        let messages = self.base().messages();
        messages.find(id)
    }

    pub fn create_message(&self, msg: &Message) -> ActResult<bool> {
        debug!("store::create_message({})", msg.id);
        let messages = self.base().messages();
        messages.create(msg)
    }

    pub fn update_message(&self, msg: &Message) -> ActResult<bool> {
        debug!("store::create_message({})", msg.id);
        let messages = self.base().messages();
        messages.update(msg)
    }

    fn base(&self) -> Arc<dyn StoreAdapter> {
        self.base.read().unwrap().clone()
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> StoreKind {
        self.kind.lock().unwrap().clone()
    }
}
