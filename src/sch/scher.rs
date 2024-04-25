use super::TaskState;
use crate::{
    cache::Cache,
    config::Config,
    event::{Action, Emitter},
    model::Workflow,
    sch::{
        queue::{Queue, Signal},
        Proc, Task,
    },
    utils::{self, consts},
    ActError, ActionResult, Error, Result, Vars,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::{
    runtime::Handle,
    time::{self, Duration},
};
use tracing::{debug, error, info};

#[derive(Clone)]
pub struct Scheduler {
    queue: Arc<Queue>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    closed: Arc<Mutex<bool>>,
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
        Scheduler::new_with(&Config::default())
    }

    pub fn new_with(options: &Config) -> Arc<Self> {
        let scher = Arc::new(Self {
            queue: Queue::new(),
            cache: Arc::new(Cache::new(options.cache_cap)),
            emitter: Arc::new(Emitter::new()),
            closed: Arc::new(Mutex::new(false)),
        });

        scher.initialize(options);
        scher
    }

    pub fn init(self: &Arc<Self>) {
        debug!("sch::init");
        self.cache.init();
    }

    pub fn start(self: &Arc<Self>, model: &Workflow, options: &Vars) -> Result<Arc<Proc>> {
        debug!("sch::start({})", model.id);

        let mut proc_id = utils::longid();
        if let Some(pid) = &options.get::<String>("pid") {
            // the pid will use as the proc_id
            proc_id = pid.to_string();
        }
        let proc = self.cache.proc(&proc_id);
        if proc.is_some() {
            return Err(ActError::Action(format!(
                "proc_id({proc_id}) is duplicated in running proc list"
            )));
        }

        // merge vars with workflow.env
        let mut w = model.clone();
        w.set_inputs(&options);

        let proc = Arc::new(Proc::new(&proc_id));
        proc.load(&w)?;
        self.launch(&proc);

        Ok(proc)
    }

    pub async fn next(self: &Arc<Self>) -> bool {
        if let Some(signal) = self.queue.next().await {
            debug!("next: {:?}", signal);
            match signal {
                Signal::Task(task) => {
                    if let Some(proc) = self.cache.proc(&task.proc_id) {
                        let task_id = task.id.clone();
                        let ctx = task.create_context();
                        Handle::current().spawn(async move {
                            proc.do_task(&task_id, &ctx);
                        });
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

    pub async fn event_loop(self: &Arc<Self>) {
        loop {
            let ret = self.next().await;
            if !ret {
                *self.closed.lock().unwrap() = true;
                break;
            }
        }
    }

    pub fn proc(&self, pid: &str) -> Option<Arc<Proc>> {
        self.cache.proc(pid)
    }

    pub fn launch(self: &Arc<Self>, proc: &Arc<Proc>) {
        debug!("sch::launch");
        let scher = self.clone();
        let proc = proc.clone();
        tokio::spawn(async move {
            proc.start(&scher);
        });
    }

    pub fn push(&self, task: &Arc<Task>) {
        debug!("sch::push  task={:?}", task);
        self.queue.send(&Signal::Task(task.clone()));
    }

    pub fn do_action(&self, action: &Action) -> Result<ActionResult> {
        match self.cache.proc(&action.proc_id) {
            Some(proc) => proc.do_action(&action),
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

    #[cfg(test)]
    pub(crate) fn signal<T: Clone>(&self, v: T) -> crate::Signal<T> {
        crate::Signal::new(v)
    }

    pub fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }

    pub fn is_closed(&self) -> bool {
        self.closed.lock().unwrap().clone()
    }

    #[allow(unused)]
    pub(crate) fn create_proc(&self, pid: &str, model: &Workflow) -> Arc<Proc> {
        let proc = Arc::new(Proc::new(pid));
        proc.load(model);
        proc
    }

    fn initialize(self: &Arc<Scheduler>, options: &Config) {
        {
            let cache = self.cache.clone();
            let emitter = self.emitter();
            emitter.init(self);

            let scher = self.clone();
            emitter.on_proc(move |proc| {
                info!("on_proc: {:?}", proc);
                let workflow_state = proc.workflow_state();
                let state = proc.state();
                if state.is_running() || state.is_pending() {
                    scher.emitter().emit_start_event(&workflow_state);
                } else {
                    if state.is_error() {
                        scher.emitter().emit_error(&workflow_state);
                    } else if state.is_completed() {
                        scher.emitter().emit_complete_event(&workflow_state);
                    }

                    // if the proc is a sub proc
                    // call the parent act
                    if let Some((ppid, ptid)) = proc.parent() {
                        scher.return_to_act(&ppid, &ptid, proc);
                    }

                    // proc.print();
                    cache.remove(&proc.id()).unwrap_or_else(|err| {
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
                }
            });
        }
        {
            let cache = self.cache.clone();
            let evt = self.emitter();
            evt.on_task(move |e| {
                info!("on_task: task={:?}", e.inner());
                cache
                    .upsert(e)
                    .unwrap_or_else(|err| error!("scher.initialize upsert={}", err));

                let ctx = e.create_context();
                // run the hook events
                e.run_hooks(&ctx)
                    .unwrap_or_else(|err| error!("scher.initialize hooks={}", err));

                // check task is allowed to emit message to client
                if e.extra().emit_message && !e.state().is_pending() {
                    let emitter = ctx.scher.emitter();
                    if !e.is_emit_disabled() {
                        let msg = e.create_message();
                        emitter.emit_message(&msg);
                    }
                }
            });
        }
        {
            let evt = self.emitter().clone();
            let cache = self.cache.clone();
            evt.on_tick(move |_| {
                for proc in cache.procs().iter() {
                    if proc.state().is_running() {
                        proc.do_tick();
                    }
                }
            });

            // start tick interval
            #[allow(unused_assignments)]
            let mut default_interval_secs = 15;
            if options.tick_interval_secs > 0 {
                #[allow(unused_assignments)]
                {
                    default_interval_secs = options.tick_interval_secs;
                }
            }
            #[cfg(test)]
            {
                default_interval_secs = 1;
            }
            Handle::current().spawn(async move {
                let mut intv = time::interval(Duration::from_secs(default_interval_secs));
                loop {
                    intv.tick().await;
                    evt.emit_tick();
                }
            });
        }
    }

    fn return_to_act(self: &Arc<Self>, pid: &str, tid: &str, proc: &Proc) {
        debug!("scher.return_to_act");
        let state = proc.state();
        // proc.print();
        let mut vars = proc.outputs();
        println!("sub outputs: {vars}");
        let mut event = consts::EVT_COMPLETE;
        if state.is_abort() {
            event = consts::EVT_ABORT;
        } else if state.is_skip() {
            event = consts::EVT_SKIP;
        } else if let TaskState::Fail(ref s) = state {
            event = consts::EVT_ERR;
            let err = Error::parse(s);
            match err.key {
                Some(key) => {
                    vars.insert(consts::ACT_ERR_CODE.to_string(), json!(key));
                }
                None => {
                    vars.insert(
                        consts::ACT_ERR_CODE.to_string(),
                        json!(consts::ACT_ERR_INNER),
                    );
                }
            }
            vars.insert(consts::ACT_ERR_MESSAGE.to_string(), json!(err.message));
        }
        let action = Action::new(pid, tid, event, &vars);
        let scher = self.clone();
        tokio::spawn(async move {
            let _ = scher
                .do_action(&action)
                .map_err(|err| error!("scher::return_to_act {}", err.to_string()));
        });
    }
}
