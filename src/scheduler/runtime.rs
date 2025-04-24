use tokio::{runtime::Handle, time};
use tracing::{debug, error};

use super::{Process, Scheduler, Task, TaskState};
use crate::event::EventAction;
use crate::{
    cache::Cache,
    data,
    env::Enviroment,
    event::Emitter,
    utils::{self, consts},
    ActError, Action, Config, Engine, Result, Vars, Workflow,
};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub struct Runtime {
    config: Arc<Config>,
    scher: Arc<Scheduler>,
    env: Arc<Enviroment>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
}

impl Runtime {
    pub fn new(config: &Config) -> Arc<Self> {
        let runtime = Self::create(config);
        runtime.event_loop();

        runtime
    }

    #[allow(unused)]
    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }

    #[allow(unused)]
    pub fn scher(&self) -> &Arc<Scheduler> {
        &self.scher
    }
    #[allow(unused)]
    pub fn env(&self) -> &Arc<Enviroment> {
        &self.env
    }

    pub fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    #[allow(unused)]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    #[allow(unused)]
    pub fn is_running(&self) -> bool {
        !self.scher.is_closed()
    }

    pub fn init(&self, engine: &Engine) {
        self.scher.init(engine);
        self.cache.init(engine);
        self.emitter.init(&engine.runtime());
    }

    pub fn start(self: &Arc<Self>, model: &Workflow, options: &Vars) -> Result<Arc<Process>> {
        debug!("scheduler::start({})", model.id);

        let mut proc_id = utils::longid();
        if let Some(pid) = &options.get::<String>("pid") {
            // the pid will use as the proc_id
            proc_id = pid.to_string();
        }
        let proc = self.cache.proc(&proc_id, self);
        if proc.is_some() {
            return Err(ActError::Action(format!(
                "proc_id({proc_id}) is duplicated in running process list"
            )));
        }

        let mut w = model.clone();
        w.set_inputs(options);

        let proc = Process::new(&proc_id, self);
        proc.load(&w)?;
        self.launch(&proc);

        Ok(proc)
    }

    pub fn proc(self: &Arc<Self>, pid: &str) -> Option<Arc<Process>> {
        self.cache.proc(pid, self)
    }

    pub fn launch(self: &Arc<Self>, proc: &Arc<Process>) {
        debug!("scheduler::launch");
        let proc = proc.clone();
        tokio::spawn(async move {
            proc.start();
        });
    }

    #[allow(unused)]
    pub(crate) fn create_proc(self: &Arc<Self>, pid: &str, model: &Workflow) -> Arc<Process> {
        let proc = Process::new(pid, self);
        proc.load(model);
        proc
    }

    pub fn push(&self, task: &Arc<Task>) {
        debug!("scheduler::push  task={:?}", task);
        self.cache
            .upsert(task)
            .unwrap_or_else(|_| panic!("fail to upsert task({})", task.id));
        self.scher.push(task);
    }

    pub fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        debug!("scheduler::do_action  action={:?}", action);
        match self.cache.proc(&action.pid, self) {
            Some(proc) => proc.do_action(action),
            None => Err(ActError::Runtime(format!(
                "cannot find process '{}' when do_action({:?})",
                action.pid, action
            ))),
        }
    }

    pub fn ack(&self, id: &str) -> Result<()> {
        self.cache
            .store()
            .set_message(id, data::MessageStatus::Acked)
    }

    pub fn event_loop(self: &Arc<Self>) {
        let scher = self.scher.clone();
        let cache = self.cache.clone();
        tokio::spawn(async move {
            loop {
                let ret = scher.next().await;
                if !ret {
                    cache.close();
                    break;
                }
            }
        });
    }

    fn create(config: &Config) -> Arc<Runtime> {
        let scher = Scheduler::new_with(config);
        let env = Arc::new(Enviroment::new());
        let cache = Arc::new(Cache::new(config.cache_cap));
        let emitter = Arc::new(Emitter::new());
        let runtime = Arc::new(Runtime {
            config: Arc::new(config.clone()),
            emitter,
            scher,
            env,
            cache,
        });

        runtime.initialize(config);
        runtime
    }

    fn initialize(self: &Arc<Self>, options: &Config) {
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.scher.on_proc(move |proc| {
                debug!("on_proc: {:?}", proc);
                if let Some(root) = proc.root() {
                    let state = proc.state();
                    let message = root.create_message();
                    if state.is_running() || state.is_pending() {
                        rt.emitter().emit_start_event(&message);
                    } else {
                        if state.is_error() {
                            rt.emitter().emit_error(&message);
                        } else if state.is_completed() {
                            rt.emitter().emit_complete_event(&message);
                        }

                        // if the process is a sub process
                        // call the parent act
                        if let Some((ppid, ptid)) = proc.parent() {
                            rt.return_to_act(&ppid, &ptid, proc);
                        }

                        if !rt.config.keep_processes {
                            debug!("remove: {:?}", proc.tasks());
                            cache.remove(proc.id()).unwrap_or_else(|err| {
                                error!("scher.initialize remove={}", err);
                                false
                            });
                            cache
                                .restore(&rt, |proc| {
                                    // println!("re-start process={process:?} tasks:{:?}", process.tasks());
                                    if proc.state().is_none() {
                                        proc.start();
                                    }
                                })
                                .unwrap_or_else(|err| error!("scher.initialize restore={}", err));
                        }
                    }
                } else {
                    error!("cannot find root pid={}", proc.id());
                    error!("tasks={:?}", proc.tasks());
                }
            });
        }
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.scher.on_task(move |e| {
                debug!("on_task: task={:?}", e.inner());
                cache
                    .upsert(e)
                    .unwrap_or_else(|err| error!("scher.initialize upsert={}", err));

                let ctx = e.create_context();
                // run the hook events
                e.run_hooks(&ctx)
                    .unwrap_or_else(|err| error!("scher.initialize hooks={}", err));

                // check task is allowed to emit message to client
                if e.extra().emit_message
                    && !e.state().is_pending()
                    && !e.state().is_running()
                    && !e.is_emit_disabled()
                {
                    let msg = e.create_message();
                    debug!("emit_message:{msg:?}");
                    rt.emitter().emit_message(&msg);
                }
            });
        }
        {
            // start tick interval
            #[allow(unused_assignments)]
            let mut default_interval_millis = 15;
            let max_message_retry_times = options.max_message_retry_times;
            if options.tick_interval_secs > 0 {
                #[allow(unused_assignments)]
                {
                    default_interval_millis = options.tick_interval_secs * 1000;
                }
            }
            #[cfg(test)]
            {
                default_interval_millis = 900;
            }

            let evt = self.emitter().clone();
            let cache = self.cache.clone();
            self.emitter().on_tick(move |_| {
                // do the process tick works
                for proc in cache.procs().iter() {
                    if proc.state().is_running() {
                        proc.do_tick();
                    }
                }

                // re-send the messages if it is neither acked nor completed
                cache.store().with_no_response_messages(
                    default_interval_millis,
                    max_message_retry_times,
                    |m| {
                        evt.emit_message(m);
                    },
                );
            });

            let evt = self.emitter().clone();
            Handle::current().spawn(async move {
                let mut intv = time::interval(Duration::from_millis(default_interval_millis));
                loop {
                    intv.tick().await;
                    evt.emit_tick();
                }
            });
        }
    }

    fn return_to_act(self: &Arc<Self>, pid: &str, tid: &str, proc: &Process) {
        debug!("scher.return_to_act");
        let state = proc.state();
        // process.print();
        let mut vars = proc.outputs();
        debug!("sub outputs: {vars}");

        let event = match state {
            TaskState::Aborted => EventAction::Abort,
            TaskState::Skipped => EventAction::Skip,
            TaskState::Error => {
                if let Some(err) = proc.err() {
                    vars.set(consts::ACT_ERR_CODE, err.ecode);
                    vars.set(consts::ACT_ERR_MESSAGE, err.message);
                }

                EventAction::Error
            }
            _ => EventAction::Next,
        };

        let action = Action::new(pid, tid, event, &vars);
        let scher = self.clone();
        tokio::spawn(async move {
            let _ = scher
                .do_action(&action)
                .map_err(|err| error!("scher::return_to_act {}", err.to_string()));
        });
    }
}
