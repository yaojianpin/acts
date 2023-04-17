use crate::{
    event::{EventData, Message},
    sch::{Proc, Task},
    ShareLock, State, Workflow,
};
use std::sync::{Arc, RwLock};
use tokio::runtime::Handle;
use tracing::{debug, info};

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

pub type ActWorkflowHandle = Arc<dyn Fn(&State<Workflow>) + Send + Sync>;
pub type ActWorkflowMessageHandle = Arc<dyn Fn(&Message) + Send + Sync>;
pub type ActProcHandle = Arc<dyn Fn(&Arc<Proc>, &EventData) + Send + Sync>;
pub type ActTaskHandle = Arc<dyn Fn(&Proc, &Task, &EventData) + Send + Sync>;

#[derive(Clone)]
pub enum Event {
    OnStart(ActWorkflowHandle),
    OnComplete(ActWorkflowHandle),
    OnMessage(ActWorkflowMessageHandle),
    OnError(ActWorkflowHandle),

    OnProc(ActProcHandle),
    OnTask(ActTaskHandle),
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OnStart(_) => f.debug_tuple("OnStart").finish(),
            Self::OnComplete(_) => f.debug_tuple("OnComplete").finish(),
            Self::OnMessage(_) => f.debug_tuple("OnMessage").finish(),
            Self::OnError(_) => f.debug_tuple("OnError").finish(),
            Self::OnProc(_) => f.debug_tuple("OnProc").finish(),
            Self::OnTask(_) => f.debug_tuple("OnTask").finish(),
        }
    }
}

unsafe impl Send for Event {}
unsafe impl Sync for Event {}

pub struct Emitter {
    starts: ShareLock<Vec<ActWorkflowHandle>>,
    completes: ShareLock<Vec<ActWorkflowHandle>>,

    messages: ShareLock<Vec<ActWorkflowMessageHandle>>,
    errors: ShareLock<Vec<ActWorkflowHandle>>,

    procs: ShareLock<Vec<ActProcHandle>>,
    tasks: ShareLock<Vec<ActTaskHandle>>,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHub").finish()
    }
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::OnStart(_) => f.write_str("ActEvent:OnStart"),
            Event::OnComplete(_) => f.write_str("ActEvent:OnComplete"),
            Event::OnMessage(_) => f.write_str("ActEvent:OnMessage"),
            Event::OnError(_) => f.write_str("ActEvent:OnError"),
            Event::OnProc(_) => f.write_str("ActEvent:OnProc"),
            Event::OnTask(_) => f.write_str("ActEvent:OnTask"),
        }
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
        }
    }

    pub fn on_message(&self, f: impl Fn(&Message) + Send + Sync + 'static) {
        let evt = Event::OnMessage(Arc::new(f));
        self.add_event(&evt);
    }

    pub fn on_start(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        let evt = Event::OnStart(Arc::new(f));
        self.add_event(&evt);
    }

    pub fn on_complete(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        let evt = Event::OnComplete(Arc::new(f));
        self.add_event(&evt);
    }

    pub fn on_error(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        let evt = Event::OnError(Arc::new(f));
        self.add_event(&evt);
    }

    pub fn on_proc(&self, f: impl Fn(&Arc<Proc>, &EventData) + Send + Sync + 'static) {
        let evt = Event::OnProc(Arc::new(f));
        self.add_event(&evt);
    }

    pub fn on_task(&self, f: impl Fn(&Proc, &Task, &EventData) + Send + Sync + 'static) {
        let evt = Event::OnTask(Arc::new(f));
        self.add_event(&evt);
    }

    pub fn dispatch_proc_event(&self, proc: &Arc<Proc>, data: &EventData) {
        debug!("dispatch_proc_event: {}", proc.pid());
        // let proc = proc.clone();
        // let data = data.clone();
        // dispatch_event!(self, procs, &proc, &data);
        let handlers = self.procs.read().unwrap();
        for handle in handlers.iter() {
            (handle)(proc, &data);
        }
    }

    pub fn dispatch_task_event(&self, proc: &Proc, task: &Task, data: &EventData) {
        debug!("dispatch_task_event: task={:?} data={:?}", task, data);
        let handlers = self.tasks.read().unwrap();
        for handle in handlers.iter() {
            (handle)(&proc, &task, &data);
        }
    }

    pub fn dispatch_start_event(&self, state: &State<Workflow>) {
        info!("dispatch_start_event: {:?}", state);
        let state = state.clone();
        dispatch_event!(self, starts, &state);
    }

    pub fn dispatch_complete_event(&self, state: &State<Workflow>) {
        info!("dispatch_complete_event: {:?}", state);
        let state = state.clone();
        dispatch_event!(self, completes, &state);
    }

    pub fn dispatch_message(&self, msg: &Message) {
        info!("dispatch_message: {:?}", msg);
        let msg = msg.clone();
        dispatch_event!(self, messages, &msg);
    }

    pub fn dispatch_error(&self, state: &State<Workflow>) {
        info!("dispatch_error: {:?}", state);
        let state = state.clone();
        dispatch_event!(self, errors, &state);
    }

    pub fn add_event(&self, evt: &Event) {
        match evt {
            Event::OnStart(func) => self.starts.write().unwrap().push(func.clone()),
            Event::OnComplete(func) => self.completes.write().unwrap().push(func.clone()),
            Event::OnMessage(func) => self.messages.write().unwrap().push(func.clone()),
            Event::OnError(func) => self.errors.write().unwrap().push(func.clone()),
            Event::OnProc(func) => self.procs.write().unwrap().push(func.clone()),
            Event::OnTask(func) => self.tasks.write().unwrap().push(func.clone()),
        }
    }
}
