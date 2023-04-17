use crate::{
    env::Enviroment,
    event::{ActionOptions, Emitter, EventAction, EventData, Message, UserMessage},
    model::Workflow,
    options::Options,
    sch::{
        cache::Cache,
        queue::{Queue, Signal},
        Context, Proc, Task, TaskState,
    },
    utils, ActError, ActResult, Engine, RuleAdapter, RwLock, ShareLock, Step,
};
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::{debug, info};

#[derive(Clone)]
pub struct Scheduler {
    queue: Arc<Queue>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    env: Arc<Enviroment>,

    engine: ShareLock<Option<Engine>>,
}

impl Scheduler {
    pub fn new() -> Arc<Self> {
        let config = utils::default_config();
        Scheduler::new_with(&config)
    }

    pub fn new_with(options: &Options) -> Arc<Self> {
        let scher = Arc::new(Self {
            queue: Queue::new(options.scher_cap),
            cache: Arc::new(Cache::new(options.cache_cap)),
            emitter: Arc::new(Emitter::new()),
            env: Arc::new(Enviroment::new()),

            engine: Arc::new(RwLock::new(None)),
        });
        Self::init_events(&scher);

        scher
    }

    pub async fn init(&self, engine: &Engine) {
        debug!("scher::init");
        *self.engine.write().unwrap() = Some(engine.clone());

        self.env.init(engine);
        self.queue.init(engine);
        self.cache.init(engine);
    }

    pub fn ord(&self, name: &str, acts: &Vec<String>) -> ActResult<Vec<String>> {
        debug!("sch::ord({})", name);
        match &*self.engine.read().unwrap() {
            Some(engine) => {
                let adapter = engine.adapter();
                adapter.ord(name, acts)
            }
            None => Err(ActError::RuntimeError("sch::engine not found".to_string())),
        }
    }

    pub fn some(&self, name: &str, step: &Step, ctx: &Context) -> ActResult<bool> {
        debug!("sch::some({})", name);
        match &*self.engine.read().unwrap() {
            Some(engine) => {
                let adapter = engine.adapter();
                adapter.some(name, step, ctx)
            }
            None => Err(ActError::RuntimeError("sch::engine not found".to_string())),
        }
    }

    pub fn start(&self, model: &Workflow, options: ActionOptions) -> ActResult<bool> {
        debug!("sch::start({})", model.id);

        let mut pid = utils::longid();
        if let Some(biz_id) = options.biz_id {
            // the biz_id will use as the pid
            pid = biz_id;
        }

        let proc = self.cache.proc(&pid);
        if proc.is_some() {
            return Err(ActError::OperateError(format!(
                "pid({pid}) is duplicated in running proc list"
            )));
        }

        // merge vars in options and workflow.env
        let mut w = model.clone();
        let mut vars = options.vars;
        for (k, v) in &w.env {
            if !vars.contains_key(k) {
                vars.insert(k.to_string(), v.clone());
            }
        }
        w.set_env(vars);

        let proc = self.create_raw_proc(&pid, &w);
        self.queue.send(&Signal::Proc(Arc::new(proc)));

        Ok(true)
    }

    pub async fn next(self: &Arc<Self>) -> bool {
        let mut handlers = Vec::new();
        if let Some(signal) = self.queue.next().await {
            debug!("signal: {:?}", signal);
            match signal {
                Signal::Proc(proc) => {
                    let scher = self.clone();
                    handlers.push(Handle::current().spawn(async move {
                        proc.start(&scher);
                    }));
                }
                Signal::Task(task) => {
                    let proc = self.cache.proc(&task.pid).expect("failed to get proc");
                    let scher = self.clone();
                    handlers.push(Handle::current().spawn(async move {
                        proc.do_task(&task.tid, &scher);
                    }));
                }
                Signal::Message(msg) => {
                    if let Some(proc) = self.cache.proc(&msg.pid) {
                        let proc = proc.clone();
                        let scher = self.clone();
                        handlers.push(Handle::current().spawn(async move {
                            proc.do_message(&msg, &scher);
                        }));
                    }
                }
                Signal::Terminal => {
                    return false;
                }
            }
        }

        return true;
    }

    pub async fn event_loop(self: &Arc<Scheduler>) {
        loop {
            let ret = self.next().await;
            if !ret {
                break;
            }
        }
    }
    pub fn sched_proc(&self, proc: &Arc<Proc>) {
        debug!("sch::sched_proc");
        self.queue.send(&Signal::Proc(proc.clone()));
    }

    pub fn sched_task(&self, task: &Task) {
        debug!("sch::sched_task  task={:?}", task);
        self.queue.send(&Signal::Task(task.clone()));
    }

    pub fn sched_message(&self, message: &UserMessage) {
        debug!("sch::sched_message");
        self.queue.send(&Signal::Message(message.clone()));
    }

    pub fn close(&self) {
        debug!("sch::close");
        self.cache.close();
        self.queue.terminate();
    }

    pub fn message(&self, id: &str) -> Option<Message> {
        self.cache.message(id)
    }

    pub fn message_by_uid(&self, pid: &str, uid: &str) -> Option<Message> {
        self.cache.message_by_uid(pid, uid)
    }

    // pub(crate) fn nearest_done_task_by_uid(&self, pid: &str, uid: &str) -> Option<Arc<Task>> {
    //     self.cache.nearest_done_task_by_uid(pid, uid)
    // }

    pub fn emitter(&self) -> Arc<Emitter> {
        self.emitter.clone()
    }

    pub fn env(&self) -> Arc<Enviroment> {
        self.env.clone()
    }

    pub fn cache(&self) -> Arc<Cache> {
        self.cache.clone()
    }

    pub(crate) fn create_raw_proc(&self, pid: &str, model: &Workflow) -> Proc {
        let scher = Arc::new(self.clone());
        let proc = Proc::new(pid, scher, &model, &TaskState::None);

        proc
    }

    fn init_events(scher: &Arc<Scheduler>) {
        {
            let scher = scher.clone();
            let cache = scher.cache.clone();
            let evt = scher.emitter();
            evt.on_proc(move |proc: &Arc<Proc>, data: &EventData| {
                info!("on_proc: {}", data);

                let state = proc.workflow_state();
                if data.action == EventAction::Create {
                    cache.create_proc(proc);
                    scher.emitter().dispatch_start_event(&state);
                } else {
                    let pid = data.pid.clone();
                    cache
                        .remove(&pid)
                        .expect(&format!("fail to remove pid={}", pid));
                    cache.restore(scher.clone());

                    if data.action == EventAction::Next || data.action == EventAction::Abort {
                        scher.emitter().dispatch_complete_event(&state);
                    } else {
                        scher.emitter().dispatch_error(&state);
                    }
                }
            });
        }
        {
            let scher = scher.clone();
            let cache = scher.cache.clone();
            let evt = scher.emitter();
            evt.on_task(move |proc: &Proc, task: &Task, data: &EventData| {
                info!(
                    "on_task: tid={}, kind={} state={} data={}",
                    task.tid,
                    task.node.kind(),
                    task.state(),
                    data
                );

                cache.upsert_task(task, data);
                // dispatch message
                if task.state() == TaskState::WaitingEvent {
                    let msg = proc.make_message(&task.tid, task.uid(), task.vars());
                    cache.create_message(&msg);
                    scher.emitter().dispatch_message(&msg);
                }
            });
        }
    }
}
