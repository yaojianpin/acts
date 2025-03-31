use crate::{
    event::Message,
    sch::{Proc, Runtime, Task},
    utils, Event, Result, ShareLock,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::runtime::Handle;
use tracing::debug;

use super::TaskExtra;
macro_rules! dispatch_event {
    ($fn:ident, $event_name:ident, $(&$item:ident), +) => {
        let handles = $fn.$event_name.clone();
        Handle::current().spawn(async move {
            let handlers = handles.read().unwrap();
            for handle in handlers.iter() {
                (handle)($(&$item),+);
            }
        });
    };
}

macro_rules! dispatch_key_event {
    ($fn:ident, $event_name:ident, $(&$item:ident), +) => {
        let handles = $fn.$event_name.clone();
        Handle::current().spawn(async move {
            let handlers = handles.read().unwrap();
            for (_, handle) in handlers.iter() {
                (handle)($(&$item),+);
            }
        });
    };
}

pub type ActWorkflowMessageHandle = Arc<dyn Fn(&Event<Message>) + Send + Sync>;
pub type ProcHandle = Arc<dyn Fn(&Event<Arc<Proc>>) + Send + Sync>;
pub type TaskHandle = Arc<dyn Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync>;
pub type TickHandle = Arc<dyn Fn(&i64) + Send + Sync>;

pub struct Emitter {
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    procs: ShareLock<Vec<ProcHandle>>,
    tasks: ShareLock<Vec<TaskHandle>>,

    ticks: ShareLock<Vec<TickHandle>>,

    runtime: ShareLock<Option<Arc<Runtime>>>,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emitter").finish()
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Emitter {
    pub fn new() -> Self {
        Self {
            messages: Arc::new(RwLock::new(HashMap::new())),
            starts: Arc::new(RwLock::new(HashMap::new())),
            completes: Arc::new(RwLock::new(HashMap::new())),
            errors: Arc::new(RwLock::new(HashMap::new())),
            procs: Arc::new(RwLock::new(Vec::new())),
            tasks: Arc::new(RwLock::new(Vec::new())),
            ticks: Arc::new(RwLock::new(Vec::new())),

            runtime: Arc::new(RwLock::new(None)),
        }
    }

    pub fn init(&self, rt: &Arc<Runtime>) {
        *self.runtime.write().unwrap() = Some(rt.clone());
    }

    #[cfg(test)]
    pub fn reset(&self) {
        self.messages.write().unwrap().clear();
        self.starts.write().unwrap().clear();
        self.completes.write().unwrap().clear();
        self.errors.write().unwrap().clear();
    }

    pub fn on_message(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.messages
            .write()
            .unwrap()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_start(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.starts
            .write()
            .unwrap()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_complete(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.completes
            .write()
            .unwrap()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_error(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.errors
            .write()
            .unwrap()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_proc(&self, f: impl Fn(&Event<Arc<Proc>>) + Send + Sync + 'static) {
        self.procs.write().unwrap().push(Arc::new(f));
    }

    pub fn on_task(&self, f: impl Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync + 'static) {
        self.tasks.write().unwrap().push(Arc::new(f));
    }

    pub fn on_tick(&self, f: impl Fn(&i64) + Send + Sync + 'static) {
        self.ticks.write().unwrap().push(Arc::new(f));
    }

    pub fn emit_proc_event(&self, proc: &Arc<Proc>) {
        debug!("emit_proc_event: {}", proc.id());
        let handlers = self.procs.read().unwrap();
        let e = &Event::new(&self.runtime.read().unwrap(), proc);
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_task_event(&self, task: &Arc<Task>) -> Result<()> {
        debug!("emit_task_event: task={:?}", task);
        let handlers = self.tasks.read().unwrap();
        let e = &Event::new_with_extra(
            &self.runtime.read().unwrap(),
            task,
            &TaskExtra { emit_message: true },
        );
        for handle in handlers.iter() {
            (handle)(e);
        }

        Ok(())
    }

    pub fn emit_task_event_with_extra(&self, task: &Arc<Task>, emit_message: bool) {
        debug!("emit_task_event: task={:?}", task);
        let handlers = self.tasks.read().unwrap();
        let e = &Event::new_with_extra(
            &self.runtime.read().unwrap(),
            task,
            &TaskExtra { emit_message },
        );
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_start_event(&self, state: &Message) {
        debug!("emit_start_event: {:?}", state);
        let e = Event::new(&self.runtime.read().unwrap(), state);
        dispatch_key_event!(self, starts, &e);
    }

    pub fn emit_complete_event(&self, state: &Message) {
        debug!("emit_complete_event: {:?}", state);
        let e = Event::new(&self.runtime.read().unwrap(), state);
        dispatch_key_event!(self, completes, &e);
    }

    pub fn emit_message(&self, msg: &Message) {
        debug!("emit_message: {:?}", msg);
        let e = Event::new(&self.runtime.read().unwrap(), msg);
        dispatch_key_event!(self, messages, &e);
    }

    pub fn emit_error(&self, state: &Message) {
        debug!("emit_error: {:?}", state);
        let e = Event::new(&self.runtime.read().unwrap(), state);
        dispatch_key_event!(self, errors, &e);
    }

    pub fn emit_tick(&self) {
        let time_millis = utils::time::time_millis();
        debug!("emit_tick {time_millis}");
        dispatch_event!(self, ticks, &time_millis);
    }

    pub fn remove(&self, key: &str) {
        let mut starts = self.starts.write().unwrap();
        if starts.contains_key(key) {
            starts.remove(key);
        }

        let mut completes = self.completes.write().unwrap();
        if completes.contains_key(key) {
            completes.remove(key);
        }

        let mut errors = self.errors.write().unwrap();
        if errors.contains_key(key) {
            errors.remove(key);
        }

        let mut messages = self.messages.write().unwrap();
        if messages.contains_key(key) {
            messages.remove(key);
        }
    }
}
