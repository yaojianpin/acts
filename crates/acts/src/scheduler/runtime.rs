use tokio::{runtime::Handle, time};
use tracing::{debug, error};

use super::{Process, Task, TaskState};
use crate::{
    ActError, Action, Config, Error, Package, Result, Vars, Workflow,
    cache::Cache,
    data,
    env::Enviroment,
    event::{Emitter, EventAction},
    scheduler::queue::{Queue, QueueData},
    store::Store,
    utils::{self, consts},
};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub struct Runtime {
    config: Arc<Config>,
    queue: Arc<Queue>,
    env: Arc<Enviroment>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    package: Arc<Package>,
}

impl Runtime {
    pub fn new(config: &Config) -> crate::Result<Arc<Self>> {
        let runtime = Self::create(config)?;
        Ok(runtime)
    }

    #[allow(unused)]
    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }

    #[allow(unused)]
    pub fn queue(&self) -> &Arc<Queue> {
        &self.queue
    }

    #[allow(unused)]
    pub fn env(&self) -> &Arc<Enviroment> {
        &self.env
    }

    pub fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    pub fn package(&self) -> &Arc<Package> {
        &self.package
    }

    pub fn store(&self) -> Arc<Store> {
        self.cache.store().clone()
    }

    #[allow(unused)]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    pub fn close(&self) {
        self.queue.abort();
    }

    pub fn start(self: &Arc<Self>, model: &Workflow, options: Vars) -> Result<Arc<Process>> {
        debug!("scheduler::start({})", model.id);

        let mut proc_id = utils::longid();
        if let Some(pid) = &options.get::<String>(consts::PROCESS_ID) {
            // the pid will use as the proc_id
            proc_id = pid.to_string();
        }
        let proc = self.cache.proc(&proc_id, self)?;
        if proc.is_some() {
            return Err(ActError::Action(format!(
                "proc_id({proc_id}) is duplicated in running process list"
            )));
        }

        // validate the options
        if !model.inputs.is_empty() {
            model
                .inputs
                .validate(&(options.to_value()))
                .map_err(|err| {
                    ActError::Model(format!(
                        "model({}) inputs validation error: {}",
                        model.id, err
                    ))
                })?;
        }

        let mut model = model.clone();
        model.set_vars(&options);

        let proc = Process::new(&proc_id, self);
        proc.load(&model)?;
        self.launch(&proc)?;

        Ok(proc)
    }

    pub fn proc(self: &Arc<Self>, pid: &str) -> Result<Option<Arc<Process>>> {
        self.cache.proc(pid, self)
    }

    pub fn launch(self: &Arc<Self>, proc: &Arc<Process>) -> Result<()> {
        debug!("scheduler::launch");
        let proc = proc.clone();
        proc.start()?;
        Ok(())
    }

    #[allow(unused)]
    pub(crate) fn create_proc(self: &Arc<Self>, pid: &str, model: &Workflow) -> Arc<Process> {
        let proc = Process::new(pid, self);
        proc.load(model);
        proc
    }

    pub fn push(&self, task: &Arc<Task>) -> Result<()> {
        debug!("scheduler::push  task={:?}", task);
        let cache = self.cache.clone();
        let task_clone = task.clone();
        cache.upsert(&task_clone)?;
        self.queue.send(&task_clone)?;
        Ok(())
    }

    pub fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        debug!("scheduler::do_action  action={:?}", action);
        let proc = self.cache.proc(&action.pid, self)?;
        match proc {
            Some(proc) => proc.do_action(action),
            None => Err(ActError::Runtime(format!(
                "cannot find process '{}' when do_action({:?})",
                action.pid, action
            ))),
        }
    }

    #[cfg(test)]
    pub fn do_action2(
        self: &Arc<Self>,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: crate::Vars,
    ) -> Result<()> {
        self.do_action(&Action::new(pid, tid, action, options))
    }

    pub fn ack(&self, id: &str) -> Result<()> {
        self.cache
            .store()
            .set_message(id, data::MessageStatus::Acked)
    }

    pub fn event_loop(self: &Arc<Self>) {
        let queue = self.queue.clone();
        tokio::spawn(async move {
            loop {
                match queue.next().await {
                    Ok(data) => match data {
                        QueueData::Task(task) => {
                            let ctx = &task.create_context();
                            if let Err(err) = task.exec(ctx) {
                                eprintln!("runtime task.exec error: {}", err);
                                task.set_err(&err.clone().into());
                                ctx.set_task(&task);
                                ctx.emit_error().ok();
                            }
                        }
                        QueueData::Abort => {
                            break;
                        }
                    },
                    Err(err) => {
                        eprintln!("runtime queue.next error: {}", err);
                        break;
                    }
                }
            }
        });
    }

    fn create(config: &Config) -> crate::Result<Arc<Runtime>> {
        // let scher = Scheduler::new();
        let env = Arc::new(Enviroment::new());
        let cache = Arc::new(Cache::new(config)?);
        let emitter = Arc::new(Emitter::new());
        let package = Arc::new(Package::new());
        let queue = Queue::new();
        let runtime = Arc::new(Runtime {
            config: Arc::new(config.clone()),
            emitter,
            // scher,
            queue,
            env,
            cache,
            package,
        });

        runtime.initialize(config)?;
        Ok(runtime)
    }

    fn initialize(self: &Arc<Self>, options: &Config) -> crate::Result<()> {
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.emitter.on_proc(move |proc| {
                debug!("on_proc: {:?}", proc);
                if let Some(root) = proc.root() {
                    let state = proc.state();
                    let mut message = root.create_message();
                    if state.is_running() || state.is_pending() {
                        let emitter = rt.emitter().clone();
                        emitter.emit_start_event(&message);
                    } else {
                        if state.is_error() {
                            let emitter = rt.emitter().clone();
                            let message = message.clone();
                            emitter.emit_error(&message);
                        } else if state.is_completed() {
                            let mut is_validation_err = false;
                            let exposes = &proc.model().exposes;
                            if !exposes.is_empty() {
                                // validate the process outputs
                                let schema = crate::ActSchema::Multiple(exposes.clone());
                                if let Err(e) = schema
                                    .validate(&(message.outputs.to_value()))
                                    .map_err(|err| {
                                        ActError::Model(format!(
                                            "model({}) outputs validation error: {}",
                                            proc.model().id,
                                            err
                                        ))
                                    })
                                {
                                    is_validation_err = true;
                                    let error = e.to_string();
                                    message.set_err("", &error);
                                    proc.set_err(&Error::new(&error, ""));
                                    let emitter = rt.emitter().clone();
                                    emitter.emit_error(&message);
                                }
                            }

                            if !is_validation_err {
                                let emitter = rt.emitter().clone();
                                emitter.emit_complete_event(&message);
                            }
                        }

                        // if the process is a sub process
                        // call the parent act
                        if let Some((ppid, ptid)) = proc.parent() {
                            rt.return_to_act(&ppid, &ptid, proc);
                        }

                        if !rt.config.keep_processes() {
                            debug!("remove: {:?}", proc.tasks());
                            let cache = cache.clone();
                            let pid = proc.id().to_string();
                            cache.remove(&pid).unwrap_or_else(|err| {
                                error!("scher.initialize remove={}", err);
                                false
                            });
                        }

                        let cache = cache.clone();
                        let rt = rt.clone();
                        cache
                            .restore(&rt)
                            .unwrap_or_else(|err| error!("scher.initialize restore={}", err));
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
            self.emitter.on_task(move |e| {
                debug!("on_task: task={:?}", e.inner());
                let cache = cache.clone();
                let e_clone = e.clone();
                cache
                    .upsert(&e_clone)
                    .unwrap_or_else(|err| error!("scher.initialize upsert={}", err));

                // check task is allowed to emit message to client
                if !e.state().is_pending() && !e.state().is_running() && e.is_emit() {
                    let msg = e.create_message();
                    debug!("emit_message:{msg:?}");
                    let emitter = rt.emitter().clone();
                    emitter.emit_message(&msg);
                }
            });
        }
        {
            // Message retry timer — periodically re-send unacknowledged messages
            let max_message_retry_times = options.max_message_retry_times();
            #[cfg(not(test))]
            let interval_ms = {
                let secs = if options.tick_interval_secs() > 0 {
                    options.tick_interval_secs()
                } else {
                    15
                };
                (secs * 1000) as u64
            };
            #[cfg(test)]
            let interval_ms = 800u64;

            let evt = self.emitter().clone();
            let cache = self.cache.clone();
            Handle::current().spawn(async move {
                let mut intv = time::interval(Duration::from_millis(interval_ms));
                loop {
                    intv.tick().await;
                    let _ = cache.store().with_no_response_messages(
                        interval_ms as i64,
                        max_message_retry_times,
                        |m| {
                            let emitter = evt.clone();
                            let m = m.clone();
                            emitter.emit_message(&m);
                        },
                    );
                }
            });
        }

        Ok(())
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

        let action = Action::new(pid, tid, event, vars);
        let scher = self.clone();
        let _ = scher
            .do_action(&action)
            .map_err(|err| error!("scher::return_to_act {}", err.to_string()));
    }
}
