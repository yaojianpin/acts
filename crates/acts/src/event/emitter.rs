use crate::{
    Event, Result, ShareLock,
    event::Message,
    scheduler::{Process, Task},
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tracing::{debug, instrument};

use super::TaskExtra;

macro_rules! dispatch_key_event {
    ($fn:ident, $event_name:ident, $item:ident) => {
        let handlers = $fn.$event_name.read().unwrap();
        for (_, handle) in handlers.iter() {
            let handle = handle.clone();
            let item = $item.clone();
            tokio::spawn(async move {
                let event = Event::from_inner(item);
                (handle)(&event);
            });
        }
    };
}

pub type ActWorkflowMessageHandle = Arc<dyn Fn(&Event<Message>) + Send + Sync>;
pub type ProcHandle = Arc<dyn Fn(&Event<Arc<Process>>) + Send + Sync>;
pub type TaskHandle = Arc<dyn Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync>;

pub struct Emitter {
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    procs: ShareLock<Vec<ProcHandle>>,
    tasks: ShareLock<Vec<TaskHandle>>,
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
        }
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

    pub fn on_proc(&self, f: impl Fn(&Event<Arc<Process>>) + Send + Sync + 'static) {
        self.procs.write().unwrap().push(Arc::new(f));
    }

    pub fn on_task(&self, f: impl Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync + 'static) {
        self.tasks.write().unwrap().push(Arc::new(f));
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub fn emit_proc_event(&self, proc: &Arc<Process>) {
        debug!("proc event emitted");
        let handlers = self.procs.read().unwrap();
        let e = &Event::new(proc);
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_task_event(&self, task: &Arc<Task>) -> Result<()> {
        self.emit_task_event_with_extra(task, true)
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub fn emit_task_event_with_extra(&self, task: &Arc<Task>, emit_message: bool) -> Result<()> {
        debug!("task event emitted");
        let handlers = self.tasks.read().unwrap();
        let e = &Event::new_with_extra(task, &TaskExtra { emit_message });
        for handle in handlers.iter() {
            (handle)(e);
        }

        Ok(())
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_start_event(&self, state: &Message) {
        debug!(state = %state.state, "start event emitted");
        dispatch_key_event!(self, starts, state);
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_complete_event(&self, state: &Message) {
        debug!(state = %state.state, "complete event emitted");
        dispatch_key_event!(self, completes, state);
    }

    #[instrument(skip(self, msg), fields(pid = %msg.pid, tid = %msg.tid, mid = %msg.mid))]
    pub fn emit_message(&self, msg: &Message) {
        debug!("message emitted");
        dispatch_key_event!(self, messages, msg);
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_error(&self, state: &Message) {
        debug!(state = %state.state, "error event emitted");
        dispatch_key_event!(self, errors, state);
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

    pub(crate) fn close(&self) {
        self.messages.write().unwrap().clear();
        self.starts.write().unwrap().clear();
        self.completes.write().unwrap().clear();
        self.errors.write().unwrap().clear();
        self.procs.write().unwrap().clear();
        self.tasks.write().unwrap().clear();
    }
}
