use crate::{
    event::{Action, Emitter, EventAction, EventData},
    model::Workflow,
    options::Options,
    sch::{
        cache::Cache,
        queue::{Queue, Signal},
        Act, Proc, Task, TaskState,
    },
    utils, ActError, ActResult, ActionState, Engine, RwLock, ShareLock, Vars,
};
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::{debug, info};

#[derive(Clone)]
pub struct Scheduler {
    queue: Arc<Queue>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    engine: ShareLock<Option<Engine>>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("count", &self.cache.count())
            .finish()
    }
}

impl Scheduler {
    pub fn new() -> Arc<Self> {
        let opt = Options::default();
        Scheduler::new_with(&opt)
    }

    pub fn new_with(options: &Options) -> Arc<Self> {
        let scher = Arc::new(Self {
            queue: Queue::new(),
            cache: Arc::new(Cache::new(options.cache_cap)),
            emitter: Arc::new(Emitter::new()),
            engine: Arc::new(RwLock::new(None)),
        });
        Self::initialize(&scher);
        scher
    }

    pub fn init(&self, engine: &Engine) {
        debug!("scher::init");
        *self.engine.write().unwrap() = Some(engine.clone());

        self.queue.init(engine);
        self.cache.init(engine);
    }

    pub fn start(&self, model: &Workflow, options: &Vars) -> ActResult<ActionState> {
        debug!("sch::start({})", model.id);

        let mut pid = utils::longid();
        if let Some(biz_id) = &options.get("biz_id") {
            // the biz_id will use as the pid
            pid = biz_id.as_str().unwrap().to_string();
        }

        let proc = self.cache.proc(&pid);
        if proc.is_some() {
            return Err(ActError::Action(format!(
                "pid({pid}) is duplicated in running proc list"
            )));
        }

        let mut state = ActionState::begin();

        // merge vars with workflow.env
        let mut w = model.clone();
        w.set_env(&options);

        let proc = Arc::new(self.create_raw_proc(&pid, &w));
        self.queue.send(&Signal::Proc(proc));

        // add pid to state
        state.insert("pid", pid.into());

        state.end();
        Ok(state)
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

    pub fn do_action(self: &Arc<Self>, act: &Action) -> ActResult<ActionState> {
        match self.cache.proc(&act.pid) {
            Some(proc) => proc.do_action(&act, self),
            None => Err(ActError::Runtime(format!("cannot find proc '{}'", act.pid))),
        }
    }

    pub fn do_ack(self: &Arc<Self>, pid: &str, aid: &str) -> ActResult<ActionState> {
        match self.cache.proc(pid) {
            Some(proc) => proc.do_ack(aid, self),
            None => Err(ActError::Runtime(format!("cannot find proc '{pid}'"))),
        }
    }

    pub fn close(&self) {
        debug!("sch::close");
        self.cache.close();
        self.queue.terminate();
    }

    pub fn act(&self, pid: &str, aid: &str) -> Option<Arc<Act>> {
        self.cache.act(pid, aid)
    }

    pub fn emitter(&self) -> Arc<Emitter> {
        self.emitter.clone()
    }

    pub fn cache(&self) -> Arc<Cache> {
        self.cache.clone()
    }

    #[allow(unused)]
    pub(crate) fn create_proc(&self, pid: &str, model: &Workflow) -> Arc<Proc> {
        let proc = Arc::new(Proc::new(pid, &model, &TaskState::None));
        self.cache.push_proc(&proc);
        proc
    }

    pub(crate) fn create_raw_proc(&self, pid: &str, model: &Workflow) -> Proc {
        Proc::new(pid, &model, &TaskState::None)
    }

    fn initialize(scher: &Arc<Scheduler>) {
        {
            let scher = scher.clone();
            let cache = scher.cache.clone();
            let evt = scher.emitter();
            evt.on_proc(move |proc: &Arc<Proc>, data: &EventData| {
                info!("on_proc: {}", data);

                let state = proc.workflow_state(&data.event);
                if data.event == EventAction::Create {
                    cache.push_proc(proc);
                    scher.emitter().dispatch_start_event(&state);
                } else {
                    let pid = data.pid.clone();
                    cache
                        .remove(&pid)
                        .expect(&format!("fail to remove pid={}", pid));
                    cache.restore();

                    if data.event == EventAction::Complete || data.event == EventAction::Abort {
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
            evt.on_task(move |task: &Task, data: &EventData| {
                info!(
                    "on_task: tid={}, kind={} state={} data={}",
                    task.tid,
                    task.node.kind(),
                    task.state(),
                    data
                );

                cache.upsert_task(task, data);

                let msg = task.create_message(&data.event);
                scher.emitter().dispatch_message(&msg);
            });
        }
        {
            let scher = scher.clone();
            let cache = scher.cache.clone();
            let evt = scher.emitter();
            evt.on_act(move |act: &Act, data: &EventData| {
                info!(
                    "on_act: tid={}, kind={} task.state={} act.state={} data={}",
                    act.tid,
                    act.task.node.kind(),
                    act.task.state(),
                    act.state(),
                    data
                );

                cache.upsert_act(act, data);
                let msg = act.create_message(&data.event);
                scher.emitter().dispatch_message(&msg);
            });
        }
    }
}
