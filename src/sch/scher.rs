use crate::{
    cache::Cache,
    event::{Action, Emitter},
    model::Workflow,
    options::Options,
    sch::{
        queue::{Queue, Signal},
        Proc, Task,
    },
    utils, ActError, ActionResult, Engine, Result, RwLock, ShareLock, Vars,
};
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::{debug, error, info};

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
            .field("cap", &self.cache.cap())
            .field("count", &self.cache.count())
            .finish()
    }
}

impl Scheduler {
    pub fn new() -> Arc<Self> {
        Scheduler::new_with(&Options::default())
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

    pub fn init(self: &Arc<Self>, engine: &Engine) {
        debug!("sch::init");
        *self.engine.write().unwrap() = Some(engine.clone());

        self.queue.init(engine);
        self.cache.init(engine);
    }

    pub fn start(&self, model: &Workflow, options: &Vars) -> Result<ActionResult> {
        debug!("sch::start({})", model.id);

        let mut proc_id = utils::longid();
        if let Some(pid) = &options.get("pid") {
            // the pid will use as the proc_id
            proc_id = pid.as_str().unwrap().to_string();
        }

        let proc = self.cache.proc(&proc_id);
        if proc.is_some() {
            return Err(ActError::Action(format!(
                "proc_id({proc_id}) is duplicated in running proc list"
            )));
        }

        let mut state = ActionResult::begin();

        // merge vars with workflow.env
        let mut w = model.clone();
        w.set_env(&options);

        let proc = Arc::new(Proc::new(&proc_id));
        proc.load(&w)?;
        self.launch(&proc);

        // add pid to state
        state.insert("pid", proc_id.into());

        state.end();
        Ok(state)
    }

    pub async fn next(self: &Arc<Self>) -> bool {
        let mut handlers = Vec::new();
        if let Some(signal) = self.queue.next().await {
            debug!("next: {:?}", signal);
            match signal {
                Signal::Proc(proc) => {
                    self.cache.push(&proc);
                    let scher = self.clone();
                    handlers.push(Handle::current().spawn(async move {
                        proc.start(&scher);
                    }));
                }
                Signal::Task(task) => {
                    if let Some(proc) = self.cache.proc(&task.proc_id) {
                        let scher = self.clone();
                        handlers.push(Handle::current().spawn(async move {
                            proc.do_task(&task.id, &scher);
                        }));
                    }
                }
                Signal::Terminal => {
                    self.cache.close();
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

    pub fn proc(&self, pid: &str) -> Option<Arc<Proc>> {
        self.cache.proc(pid)
    }

    pub fn launch(&self, proc: &Arc<Proc>) {
        debug!("sch::launch");
        self.queue.send(&Signal::Proc(proc.clone()));
    }

    pub fn push(&self, task: &Task) {
        debug!("sch::push  task={:?}", task);
        self.queue.send(&Signal::Task(task.clone()));
    }

    pub fn do_action(self: &Arc<Self>, action: &Action) -> Result<ActionResult> {
        match self.cache.proc(&action.proc_id) {
            Some(proc) => proc.do_action(&action, self),
            None => Err(ActError::Runtime(format!(
                "cannot find proc '{}'",
                action.proc_id
            ))),
        }
    }

    pub fn close(&self) {
        debug!("sch::close");
        self.queue.terminate();
    }

    pub fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }

    #[allow(unused)]
    pub(crate) fn create_proc(&self, pid: &str, model: &Workflow) -> Arc<Proc> {
        let proc = Arc::new(Proc::new(pid));
        proc.load(model);
        proc
    }

    fn initialize(scher: &Arc<Scheduler>) {
        {
            let cache = scher.cache.clone();
            let evt = scher.emitter();
            evt.init(scher);

            let scher = scher.clone();
            evt.on_proc(move |proc| {
                info!("on_proc: {:?}", proc);

                let workflow_state = proc.workflow_state();
                let state = proc.state();
                if state.is_running() || state.is_pending() {
                    scher.emitter().emit_start_event(&workflow_state);
                } else {
                    // proc.print();
                    let pid = proc.id();
                    cache.remove(&pid).unwrap_or_else(|err| {
                        error!("scher.initialize remove={}", err);
                        false
                    });

                    cache
                        .restore(|proc| {
                            if proc.state().is_none() {
                                proc.start(&scher);
                            }
                        })
                        .unwrap_or_else(|err| error!("scher.initialize restore={}", err));

                    if state.is_error() {
                        scher.emitter().emit_error(&workflow_state);
                    } else if state.is_completed() {
                        scher.emitter().emit_complete_event(&workflow_state);
                    }
                }
            });
        }
        {
            let cache = scher.cache.clone();
            let evt = scher.emitter();
            let scher = scher.clone();
            evt.on_task(move |e| {
                info!("on_task: task={:?}", e.inner());

                cache
                    .upsert(e)
                    .unwrap_or_else(|err| error!("scher.initialize upsert={}", err));

                // check task is allowed to emit message to client
                if e.extra().emit_message {
                    let emitter = scher.emitter();
                    if !e.is_emit_disabled() {
                        let msg = e.create_message();
                        emitter.emit_message(&msg);
                    }

                    let workflow = e.proc().model();
                    if let Some(actions) = workflow.actions(e) {
                        for action in actions {
                            let msg = e.create_action_message(action);
                            emitter.emit_message(&msg);
                        }
                    }
                }
            });
        }
    }
}
