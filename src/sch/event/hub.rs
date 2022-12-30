use crate::{
    debug,
    sch::{
        event::{message::Message, EventData},
        proc::{Proc, Task},
    },
    Act, Engine, Job, ShareLock, Step, TaskState, Workflow,
};
use std::sync::{Arc, RwLock};
use tokio::runtime::Handle;

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

pub type ActWorkflowHandle = Arc<dyn Fn(&Workflow) + Send + Sync>;
pub type ActWorkflowMessageHandle = Arc<dyn Fn(&Message) + Send + Sync>;
pub type ActJobHandle = Arc<dyn Fn(&Job) + Send + Sync>;
pub type ActStepHandle = Arc<dyn Fn(&Step) + Send + Sync>;
pub type ActActHandle = Arc<dyn Fn(&Act) + Send + Sync>;
pub type ActProcHandle = Arc<dyn Fn(&Proc, &EventData) + Send + Sync>;
pub type ActTaskHandle = Arc<dyn Fn(&Task, &EventData) + Send + Sync>;

#[derive(Clone)]
pub enum Event {
    OnStart(ActWorkflowHandle),
    OnComplete(ActWorkflowHandle),
    OnMessage(ActWorkflowMessageHandle),

    OnJob(ActJobHandle),

    OnStep(ActStepHandle),
    OnAct(ActActHandle),

    // OnPreBranch(ActBranchHandle),
    // OnPostBranch(ActBranchHandle),
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
            Self::OnJob(_) => f.debug_tuple("OnJob").finish(),
            Self::OnStep(_) => f.debug_tuple("OnStep").finish(),
            Self::OnAct(_) => f.debug_tuple("OnAct").finish(),
            Self::OnError(_) => f.debug_tuple("OnError").finish(),
            Self::OnProc(_) => f.debug_tuple("OnProc").finish(),
            Self::OnTask(_) => f.debug_tuple("OnTask").finish(),
        }
    }
}

unsafe impl Send for Event {}
unsafe impl Sync for Event {}

pub struct EventHub {
    starts: ShareLock<Vec<ActWorkflowHandle>>,
    completes: ShareLock<Vec<ActWorkflowHandle>>,

    messages: ShareLock<Vec<ActWorkflowMessageHandle>>,
    errors: ShareLock<Vec<ActWorkflowHandle>>,

    jobs: ShareLock<Vec<ActJobHandle>>,
    steps: ShareLock<Vec<ActStepHandle>>,
    acts: ShareLock<Vec<ActActHandle>>,

    procs: ShareLock<Vec<ActProcHandle>>,
    tasks: ShareLock<Vec<ActTaskHandle>>,
}

impl std::fmt::Debug for EventHub {
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
            Event::OnJob(_) => f.write_str("ActEvent:OnJob"),
            Event::OnStep(_) => f.write_str("ActEvent:OnStep"),
            Event::OnAct(_) => f.write_str("ActEvent:OnAct"),
            Event::OnError(_) => f.write_str("ActEvent:OnError"),
            Event::OnProc(_) => f.write_str("ActEvent:OnProc"),
            Event::OnTask(_) => f.write_str("ActEvent:OnTask"),
        }
    }
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            messages: Arc::new(RwLock::new(Vec::new())),
            starts: Arc::new(RwLock::new(Vec::new())),
            completes: Arc::new(RwLock::new(Vec::new())),
            errors: Arc::new(RwLock::new(Vec::new())),

            jobs: Arc::new(RwLock::new(Vec::new())),
            steps: Arc::new(RwLock::new(Vec::new())),
            acts: Arc::new(RwLock::new(Vec::new())),

            procs: Arc::new(RwLock::new(Vec::new())),
            tasks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn init(&self, engine: &Engine) {
        debug!("event::init");
        let evts = engine.evts();
        for evt in evts.iter() {
            self.add_event(evt);
        }
    }

    pub fn add_event(&self, evt: &Event) {
        debug!("add_event: {}", evt);
        match evt {
            Event::OnStart(func) => self.starts.write().unwrap().push(func.clone()),
            Event::OnComplete(func) => self.completes.write().unwrap().push(func.clone()),
            Event::OnMessage(func) => {
                self.messages.write().unwrap().push(func.clone());
            }
            Event::OnError(func) => self.errors.write().unwrap().push(func.clone()),
            Event::OnJob(func) => self.jobs.write().unwrap().push(func.clone()),
            Event::OnStep(func) => self.steps.write().unwrap().push(func.clone()),
            Event::OnAct(func) => self.acts.write().unwrap().push(func.clone()),
            Event::OnProc(func) => self.procs.write().unwrap().push(func.clone()),
            Event::OnTask(func) => self.tasks.write().unwrap().push(func.clone()),
        }
    }

    pub(crate) fn disp_proc(&self, proc: &Proc, data: &EventData) {
        debug!("disp_proc: {}", proc.pid());
        let proc = proc.clone();
        let data = data.clone();
        dispatch_event!(self, procs, &proc, &data);
    }

    pub(crate) fn disp_task(&self, task: &Task, data: &EventData) {
        debug!("disp_task: {}", task.tid());
        let task = task.clone();
        let data = data.clone();
        dispatch_event!(self, tasks, &task, &data);
    }

    pub(crate) fn disp_start(&self, workflow: &Workflow) {
        debug!("disp_start: {}", workflow.id);
        let workflow = workflow.clone();
        dispatch_event!(self, starts, &workflow);
    }
    pub(crate) fn disp_complete(&self, workflow: &Workflow) {
        debug!("disp_complete: {}", workflow.id);
        let workflow = workflow.clone();
        dispatch_event!(self, completes, &workflow);
    }
    pub(crate) fn disp_message(&self, msg: &Message) {
        debug!("disp_message: {}", msg);
        let msg = msg.clone();
        dispatch_event!(self, messages, &msg);
    }

    pub(crate) fn disp_workflow(&self, workflow: &Workflow) {
        debug!("disp_workflow: {}", workflow.id);
        let state = workflow.state();
        if state == TaskState::Running {
            self.disp_start(workflow);
        }

        if state.is_completed() {
            self.disp_complete(workflow);

            if state.is_error() {
                self.disp_error(workflow);
            }
        }
    }

    pub(crate) fn disp_job(&self, _job: &Job) {
        debug!("disp_job: {}", _job.id);
    }

    pub(crate) fn disp_step(&self, _step: &Step) {
        debug!("disp_step: {}", _step.id);
    }

    pub(crate) fn disp_act(&self, _act: &Act) {
        debug!("disp_act: {}", _act.id);
    }

    pub(crate) fn disp_error(&self, workflow: &Workflow) {
        debug!("disp_error: {}", workflow.id);
        let workflow = workflow.clone();
        dispatch_event!(self, errors, &workflow);
    }
}
