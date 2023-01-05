use crate::{
    debug,
    env::Enviroment,
    model::Workflow,
    options::Options,
    sch::{
        cache::Cache,
        event::{EventData, EventHub},
        queue::{Queue, Signal},
        Context, Event, Proc, Task, TaskState,
    },
    utils, ActError, ActResult, Engine, Message, RuleAdapter, RwLock, ShareLock, Step,
};
use std::sync::Arc;
use tokio::runtime::Handle;

#[derive(Clone)]
pub struct Scheduler {
    queue: Arc<Queue>,
    cache: Arc<Cache>,
    evt: Arc<EventHub>,
    env: Arc<Enviroment>,

    engine: ShareLock<Option<Engine>>,
}

impl Scheduler {
    pub fn new() -> Self {
        let config = utils::default_config();
        Scheduler::new_with(&config)
    }

    pub fn new_with(options: &Options) -> Self {
        Scheduler {
            queue: Queue::new(options.scher_cap),
            cache: Arc::new(Cache::new(options.cache_cap)),
            evt: Arc::new(EventHub::new()),
            env: Arc::new(Enviroment::new()),

            engine: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn init(&self, engine: &Engine) {
        debug!("scher::init");
        *self.engine.write().unwrap() = Some(engine.clone());

        self.env.init(engine);
        self.queue.init(engine);
        self.cache.init(engine);
        self.evt.init(engine);
    }

    pub fn ord(&self, name: &str, acts: &Vec<String>) -> ActResult<Vec<String>> {
        debug!("sch::ord({})", name);
        match &*self.engine.read().unwrap() {
            Some(engine) => {
                let adapter = engine.adapter();
                adapter.ord(name, acts)
            }
            None => Err(ActError::ScherError("sch::engine not found".to_string())),
        }
    }

    pub fn some(&self, name: &str, step: &Step, ctx: &Context) -> ActResult<bool> {
        debug!("sch::some({})", name);
        match &*self.engine.read().unwrap() {
            Some(engine) => {
                let adapter = engine.adapter();
                adapter.some(name, step, ctx)
            }
            None => Err(ActError::ScherError("sch::engine not found".to_string())),
        }
    }

    pub fn push(&self, workflow: &Workflow) {
        debug!("sch::push({})", workflow.id);
        let proc = self.create_raw_proc(workflow);
        self.cache.push(&proc);
        self.queue.send(&Signal::Proc(proc.clone()));
    }

    pub async fn next(&self) -> bool {
        debug!("sch::next");
        let mut handlers = Vec::new();
        if let Some(signal) = self.queue.next().await {
            match signal {
                Signal::Proc(proc) => {
                    handlers.push(Handle::current().spawn(async move {
                        proc.start();
                    }));
                }
                Signal::Task(tid, proc) => {
                    handlers.push(
                        Handle::current().spawn(async move { proc.run_with_task(&tid).await }),
                    );
                }
                Signal::Message(msg) => {
                    if let Some(proc) = self.cache.proc(&msg.pid) {
                        let proc = proc.clone();
                        handlers.push(
                            Handle::current().spawn(async move { proc.run_with_message(&msg) }),
                        );
                    }
                }
                Signal::Terminal => {
                    return false;
                }
            }
        }

        return true;
    }

    pub fn sched_proc(&self, proc: &Proc) {
        debug!("sch::sched_proc");
        self.queue.send(&Signal::Proc(proc.clone()));
    }

    pub fn sched_task(&self, proc: &Proc, tid: &str) {
        debug!("sch::sched_task");
        self.queue
            .send(&Signal::Task(tid.to_string(), proc.clone()));
    }

    pub fn sched_message(&self, message: &Message) {
        debug!("sch::sched_message");
        self.queue.send(&Signal::Message(message.clone()));
    }

    pub fn close(&self) {
        debug!("sch::close");
        self.queue.terminate();
        self.cache.close();
    }

    pub(crate) fn message(&self, id: &str) -> Option<Message> {
        self.cache.message(id)
    }

    pub(crate) fn message_by_uid(&self, pid: &str, uid: &str) -> Option<Message> {
        self.cache.message_by_uid(pid, uid)
    }

    pub fn evt(&self) -> Arc<EventHub> {
        self.evt.clone()
    }

    pub fn env(&self) -> Arc<Enviroment> {
        self.env.clone()
    }

    pub fn cache(&self) -> Arc<Cache> {
        self.cache.clone()
    }

    pub(crate) fn create_raw_proc(&self, workflow: &Workflow) -> Proc {
        debug!("sch::create_raw_proc");
        let state = &TaskState::None;
        workflow.set_state(state);

        let scher = Arc::new(self.clone());
        let proc = Proc::new(scher, &workflow, state);

        proc
    }

    pub(crate) fn on_proc(&self, f: impl Fn(&Proc, &EventData) + Send + Sync + 'static) {
        let evt = Event::OnProc(Arc::new(f));
        self.evt.add_event(&evt);
    }

    pub(crate) fn on_task(&self, f: impl Fn(&Task, &EventData) + Send + Sync + 'static) {
        let evt = Event::OnTask(Arc::new(f));
        self.evt.add_event(&evt);
    }

    pub(crate) fn on_message(&self, f: impl Fn(&Message) + Send + Sync + 'static) {
        let evt = Event::OnMessage(Arc::new(f));
        self.evt.add_event(&evt);
    }
}
