use crate::{
    debug,
    env::Enviroment,
    model::Workflow,
    options::Options,
    sch::{
        cache::Cache,
        event::EventHub,
        queue::{Queue, Signal},
        Context, Proc, Task, TaskState,
    },
    utils, ActError, ActResult, ActionOptions, Engine, Message, RuleAdapter, RwLock, ShareLock,
    Step, UserMessage,
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

    pub fn init(&self, engine: &Engine) {
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

    pub fn start(&self, id: &str, options: ActionOptions) -> ActResult<bool> {
        debug!("sch::start({})", id);
        match &options.biz_id {
            Some(biz_id) => {
                if biz_id.is_empty() {
                    return Err(ActError::OperateError("biz_id is empty in options".into()));
                }

                // the biz_id will use as the pid
                // so just check the biz_id before start a new proc
                let proc = self.cache.proc(&biz_id);
                if proc.is_some() {
                    return Err(ActError::OperateError(format!(
                        "biz_id({biz_id}) is duplicated in running proc list"
                    )));
                }

                let model = self.cache.model(id)?;
                let mut w = model.workflow()?;
                // merge vars in options and workflow.env
                let mut vars = options.vars;
                for (k, v) in &w.env {
                    if !vars.contains_key(k) {
                        vars.insert(k.to_string(), v.clone());
                    }
                }
                w.set_env(vars);

                let proc = self.create_raw_proc(biz_id, &w.clone());
                self.queue.send(&Signal::Proc(proc.clone()));

                Ok(true)
            }
            None => Err(ActError::OperateError("not found biz_id in options".into())),
        }
    }

    pub async fn next(&self) -> bool {
        debug!("sch::next");
        let mut handlers = Vec::new();
        if let Some(signal) = self.queue.next().await {
            debug!("signal: {:?}", signal);
            match signal {
                Signal::Proc(proc) => {
                    handlers.push(Handle::current().spawn(async move {
                        proc.start();
                    }));
                }
                Signal::Task(task) => {
                    let proc = self.cache.proc(&task.pid).expect("failed to get proc");
                    handlers.push(Handle::current().spawn(async move {
                        proc.do_task(&task.tid);
                    }));
                }
                Signal::Message(msg) => {
                    if let Some(proc) = self.cache.proc(&msg.pid) {
                        let proc = proc.clone();
                        handlers.push(Handle::current().spawn(async move {
                            proc.do_message(&msg);
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

    pub async fn event_loop(&self) {
        loop {
            let ret = self.next().await;
            if !ret {
                break;
            }
        }
    }
    pub fn sched_proc(&self, proc: &Proc) {
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
        self.queue.terminate();
        self.cache.close();
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

    pub fn evt(&self) -> Arc<EventHub> {
        self.evt.clone()
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
}
