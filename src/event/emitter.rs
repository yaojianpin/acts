use crate::{
    event::Message,
    sch::{Proc, Scheduler, Task},
    Event, ShareLock, WorkflowState,
};
use std::sync::{Arc, RwLock};
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

pub type ActWorkflowHandle = Arc<dyn Fn(&Event<WorkflowState>) + Send + Sync>;
pub type ActWorkflowMessageHandle = Arc<dyn Fn(&Event<Message>) + Send + Sync>;
pub type ProcHandle = Arc<dyn Fn(&Event<Arc<Proc>>) + Send + Sync>;
pub type TaskHandle = Arc<dyn Fn(&Event<Task, TaskExtra>) + Send + Sync>;

pub struct Emitter {
    starts: ShareLock<Vec<ActWorkflowHandle>>,
    completes: ShareLock<Vec<ActWorkflowHandle>>,

    messages: ShareLock<Vec<ActWorkflowMessageHandle>>,
    errors: ShareLock<Vec<ActWorkflowHandle>>,

    procs: ShareLock<Vec<ProcHandle>>,
    tasks: ShareLock<Vec<TaskHandle>>,

    scher: ShareLock<Option<Arc<Scheduler>>>,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHub").finish()
    }
}

impl Emitter {
    pub fn new() -> Self {
        Self {
            messages: Arc::new(RwLock::new(Vec::new())),
            starts: Arc::new(RwLock::new(Vec::new())),
            completes: Arc::new(RwLock::new(Vec::new())),
            errors: Arc::new(RwLock::new(Vec::new())),
            procs: Arc::new(RwLock::new(Vec::new())),
            tasks: Arc::new(RwLock::new(Vec::new())),

            scher: Arc::new(RwLock::new(None)),
        }
    }

    pub fn init(&self, scher: &Arc<Scheduler>) {
        *self.scher.write().unwrap() = Some(scher.clone());
    }

    pub fn on_message(&self, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        self.messages.write().unwrap().push(Arc::new(f));
    }

    pub fn on_start(&self, f: impl Fn(&Event<WorkflowState>) + Send + Sync + 'static) {
        self.starts.write().unwrap().push(Arc::new(f));
    }

    pub fn on_complete(&self, f: impl Fn(&Event<WorkflowState>) + Send + Sync + 'static) {
        self.completes.write().unwrap().push(Arc::new(f));
    }

    pub fn on_error(&self, f: impl Fn(&Event<WorkflowState>) + Send + Sync + 'static) {
        self.errors.write().unwrap().push(Arc::new(f));
    }

    pub fn on_proc(&self, f: impl Fn(&Event<Arc<Proc>>) + Send + Sync + 'static) {
        self.procs.write().unwrap().push(Arc::new(f));
    }

    pub fn on_task(&self, f: impl Fn(&Event<Task, TaskExtra>) + Send + Sync + 'static) {
        self.tasks.write().unwrap().push(Arc::new(f));
    }

    pub fn emit_proc_event(&self, proc: &Arc<Proc>) {
        debug!("emit_proc_event: {}", proc.id());
        let handlers = self.procs.read().unwrap();
        let e = &Event::new(&*self.scher.read().unwrap(), proc);
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_task_event(&self, task: &Task) {
        debug!("emit_task_event: task={:?}", task);
        let handlers = self.tasks.read().unwrap();
        let e = &Event::new_with_extra(
            &*self.scher.read().unwrap(),
            task,
            &TaskExtra { emit_message: true },
        );
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_task_event_with_extra(&self, task: &Task, emit_message: bool) {
        debug!("emit_task_event: task={:?}", task);
        let handlers = self.tasks.read().unwrap();
        let e = &Event::new_with_extra(
            &*self.scher.read().unwrap(),
            task,
            &TaskExtra {
                emit_message,
                ..Default::default()
            },
        );
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_start_event(&self, state: &WorkflowState) {
        debug!("emit_start_event: {:?}", state);
        let e = Event::new(&*self.scher.read().unwrap(), state);
        dispatch_event!(self, starts, &e);
    }

    pub fn emit_complete_event(&self, state: &WorkflowState) {
        debug!("emit_complete_event: {:?}", state);
        let e = Event::new(&*self.scher.read().unwrap(), state);
        dispatch_event!(self, completes, &e);
    }

    pub fn emit_message(&self, msg: &Message) {
        debug!("emit_message: {:?}", msg);
        let e = Event::new(&*self.scher.read().unwrap(), msg);
        dispatch_event!(self, messages, &e);
    }

    pub fn emit_error(&self, state: &WorkflowState) {
        debug!("emit_error: {:?}", state);
        let e = Event::new(&*self.scher.read().unwrap(), state);
        dispatch_event!(self, errors, &e);
    }
}
