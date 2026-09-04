use super::{ActTask, Process, Sign, Task, TaskState};
use crate::{
    ActError, Action, Config, Error, Package, Result, ShareLock, Vars, Workflow,
    cache::Cache,
    config::ConfigResolver,
    data,
    env::Enviroment,
    event::{Emitter, EventAction},
    scheduler::queue::{Queue, QueueData},
    store::{KvStore, Store},
    utils::{self, consts},
};
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{runtime::Handle, time};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument};

#[derive(Clone)]
pub struct Runtime {
    config: Arc<Config>,
    queue: Arc<Queue>,
    env: Arc<Enviroment>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    package: Arc<Package>,
    shutdown: CancellationToken,
    pub(crate) resolvers: ShareLock<HashMap<String, Arc<dyn ConfigResolver>>>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("config", &self.config)
            .field("queue", &self.queue)
            .field("env", &self.env)
            .field("cache", &self.cache)
            .field("emitter", &self.emitter)
            .field("package", &self.package)
            .field(
                "resolvers",
                &format_args!("<{} entries>", self.resolvers.read().len()),
            )
            .finish()
    }
}

impl Runtime {
    pub fn new(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Arc<Self>> {
        let runtime = Self::create(config, store)?;
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
    pub fn register_resolver(&self, name: &str, resolver: Arc<dyn ConfigResolver>) {
        let mut resolvers = self.resolvers.write();
        resolvers.insert(name.to_string(), resolver);
    }

    pub fn close(&self) {
        self.shutdown.cancel();
        self.queue.abort();
        self.cache.close();
        self.emitter.close();
    }

    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    #[instrument(skip(self, model, options), fields(mid = %model.id, name = %model.name))]
    pub fn start(self: &Arc<Self>, model: &Workflow, options: Vars) -> Result<Arc<Process>> {
        debug!("process starting");

        let mut proc_id = utils::longid();
        if let Some(pid) = &options.get::<String>(consts::PROCESS_ID) {
            // the pid will use as the proc_id
            proc_id = pid.to_string();

            // check external pid is valid
            if proc_id.is_empty() {
                return Err(ActError::Action(
                    "external process id cannot be empty".to_string(),
                ));
            }

            if proc_id.contains(consts::KEY_SEP) {
                return Err(ActError::Action(format!(
                    "external process id cannot contain '{}'",
                    consts::KEY_SEP
                )));
            }
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

        info!(pid = %proc_id, mid = %model.id, name = %model.name, "process started");
        Ok(proc)
    }

    pub fn proc(self: &Arc<Self>, pid: &str) -> Result<Option<Arc<Process>>> {
        self.cache.proc(pid, self)
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub fn launch(self: &Arc<Self>, proc: &Arc<Process>) -> Result<()> {
        debug!("process launched");
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

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub fn push(&self, task: &Arc<Task>) -> Result<()> {
        debug!("task pushed");
        let cache = self.cache.clone();
        let task_clone = task.clone();
        cache.upsert_async(&task_clone)?;
        self.queue.send(&task_clone)?;
        Ok(())
    }

    #[instrument(skip(self, action), fields(pid = %action.pid, tid = %action.tid, event = ?action.event))]
    pub fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        debug!("action received");
        let proc = self.cache.proc(&action.pid, self)?;
        match proc {
            Some(proc) => proc.do_action(action),
            None => Err(ActError::Runtime(format!(
                "cannot find process '{}' when do_action({:?})",
                action.pid, action
            ))),
        }
    }

    /// Durable outbox enqueue for a `next` operation: a `Pending` outbox record
    /// is queued on the store writer (after the task state change, so the task
    /// is durable first) and the operation is dispatched to the in-memory
    /// queue — neither blocks the caller. A crash before the record lands is
    /// consistent (nothing to replay); a crash after it lands is recovered by
    /// [`Self::recover_actions`]; a crash after the operation ran is a no-op
    /// thanks to the durably persisted `NEXT_COMPLETE` marker.
    pub(crate) fn enqueue_next(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.enqueue_next(task)?;
        self.queue.send_next(task)?;
        Ok(())
    }

    /// Durable outbox close for a task whose `next` propagation finished: queue
    /// the task persist (with the `NEXT_COMPLETE` marker) and then the outbox
    /// record close, in order, on the store writer — non-blocking. Called from
    /// `Task::next` once the task reaches a terminal state (also for the
    /// idempotent replay guard), and from the event loop when `next` ends in
    /// error. Non-terminal outcomes (children in flight, interrupt) leave the
    /// record `Pending` so recovery replays it.
    pub(crate) fn complete_next(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.complete_next(task)
    }

    /// Durable outbox enqueue for a client action (non-`Next` events): the
    /// `Pending` record with the event + options payload is written **before**
    /// the action is applied, so a crash before the task state write lands is
    /// replayed by [`Self::recover_actions`].
    pub(crate) fn enqueue_action(&self, action: &Action) -> Result<()> {
        self.cache.enqueue_action(action)
    }

    /// Durable outbox close for a client action: the state write and message
    /// status were already queued by the caller, so FIFO order makes `Done`
    /// durable only after both.
    pub(crate) fn complete_action(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.complete_action(task)
    }

    /// Replay durable outbox records that were not durably completed (the
    /// engine crashed before the queued `next` ran, before its effects were
    /// persisted, or before a client action's state write became durable).
    /// Re-enqueueing is idempotent:
    /// - `next` records of a task whose `next` already completed are skipped
    ///   by the durable `NEXT_COMPLETE` guard and closed;
    /// - `next` records of a task whose `next` never ran are dispatched again,
    ///   and re-scheduling is deduplicated against tasks created before the
    ///   crash;
    /// - action records of a task that is already in a terminal state are
    ///   closed (the action was applied durably) and the task's messages are
    ///   marked completed so the client is not asked to act again — except
    ///   `Cancel`/`Remove`, which never guard on the target's state (a Cancel
    ///   target is usually already `Completed`), so they are always re-applied
    ///   and an already-applied one is rejected by the arm's guards;
    /// - action records of a task that never received the action are
    ///   re-applied, which also closes the record through the action path.
    pub fn recover_actions(self: &Arc<Self>) -> Result<()> {
        let ops = self.cache.store().load_pending_ops()?;
        for op in ops {
            let r#type = op.r#type.clone();
            let (pid, tid) = (op.pid, op.tid);
            let close = |store: &Store| store.complete_ops(&pid, &tid, &r#type);
            let Some(proc) = self.cache.proc(&pid, self)? else {
                // process is gone (removed while completing) — drop the orphan
                close(&self.cache.store())?;
                continue;
            };
            let Some(task) = proc.task(&tid) else {
                close(&self.cache.store())?;
                continue;
            };
            if r#type == data::OpType::Action.as_ref() {
                let (Some(event), Some(options)) = (op.event.as_deref(), op.options.as_deref())
                else {
                    // malformed action record — drop it
                    close(&self.cache.store())?;
                    continue;
                };
                let Ok(event) = EventAction::parse(event) else {
                    error!(pid = %pid, tid = %tid, event = %event, "cannot parse replayed action");
                    close(&self.cache.store())?;
                    continue;
                };
                let Ok(options) = serde_json::from_str::<Vars>(options) else {
                    error!(pid = %pid, tid = %tid, "cannot parse replayed action options");
                    close(&self.cache.store())?;
                    continue;
                };
                // `Cancel` and `Remove` never guard on the target task's state
                // (a Cancel target is usually already `Completed` from an
                // earlier `Next`; Remove has no guard at all), so a terminal
                // target does NOT prove the action was applied — always
                // re-apply them. Re-applying an already-applied one is
                // rejected by the arm's guards and closes the record.
                let always_reapply = matches!(event, EventAction::Cancel | EventAction::Remove);
                if !always_reapply && task.state().is_completed() {
                    // already applied durably (the state write landed but the
                    // close was lost) — close and mark the messages completed
                    close(&self.cache.store())?;
                    self.cache.store().set_message_with(
                        &pid,
                        &tid,
                        data::MessageStatus::Completed,
                    )?;
                    continue;
                }
                // the action was never durably applied — re-apply it; the
                // action path (Task::update) closes the record itself
                let action = Action::new(&pid, &tid, event, options);
                if let Err(err) = proc.do_action(&action) {
                    error!(error = %err, pid = %pid, tid = %tid, "replayed action failed");
                    close(&self.cache.store())?;
                }
            } else if task.is_sign(Sign::NEXT_COMPLETE) {
                // propagation already completed durably; just close the record
                // and mark the messages completed
                close(&self.cache.store())?;
                self.cache
                    .store()
                    .set_message_with(&pid, &tid, data::MessageStatus::Completed)?;
                continue;
            } else {
                self.queue.send_next(&task)?;
            }
        }
        Ok(())
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
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    next = queue.next() => next,
                };
                match next {
                    Ok(data) => match data {
                        QueueData::Task(task) => {
                            let ctx = &task.create_context();
                            if let Err(err) = task.exec(ctx).await {
                                error!(error = %err, "task.exec failed");
                                task.set_err(&err.clone().into());
                                ctx.set_task(&task);
                                ctx.emit_error().ok();
                            }
                        }
                        QueueData::Next(task) => {
                            let ctx = &task.create_context();
                            let result = task.next(ctx).await;
                            if let Err(err) = result {
                                error!(error = %err, "task.next failed");
                                task.set_err(&err.clone().into());
                                ctx.set_task(&task);
                                ctx.emit_error().ok();
                                // the propagation ended in error (terminal):
                                // close the outbox record so recovery does not
                                // replay the failed `next`
                                if let Err(err) = task.runtime().complete_next(&task) {
                                    error!(error = %err, "complete_next failed");
                                }
                            }
                            // on success the record is closed inside `next`
                            // once the task reaches a terminal state; outcomes
                            // with children still in flight or an interrupt
                            // leave it `Pending` for recovery to replay.
                        }
                        QueueData::Abort => {
                            break;
                        }
                    },
                    Err(err) => {
                        error!(error = %err, "queue.next failed");
                        break;
                    }
                }
            }
        });
    }

    fn create(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Arc<Runtime>> {
        // let scher = Scheduler::new();
        let env = Arc::new(Enviroment::new());
        let cache = Arc::new(Cache::new(config, store)?);
        let emitter = Arc::new(Emitter::new());
        let package = Arc::new(Package::new());
        let queue = Queue::new();
        let shutdown = CancellationToken::new();
        let resolvers = Arc::new(RwLock::new(HashMap::new()));
        let runtime = Arc::new(Runtime {
            config: Arc::new(config.clone()),
            emitter,
            // scher,
            queue,
            env,
            cache,
            package,
            shutdown,
            resolvers,
        });

        runtime.initialize()?;
        Ok(runtime)
    }

    fn initialize(self: &Arc<Self>) -> crate::Result<()> {
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.emitter.on_proc(move |proc| {
                debug!(pid = %proc.id(), "proc event");
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
                        let final_state = proc.state();
                        if final_state.is_error() {
                            info!(pid = %proc.id(), state = %final_state, cost_ms = proc.cost(), "process errored");
                        } else if final_state.is_completed() {
                            info!(pid = %proc.id(), state = %final_state, cost_ms = proc.cost(), "process completed");
                        }

                        // if the process is a sub process
                        // call the parent act
                        if let Some((ppid, ptid)) = proc.parent() {
                            rt.return_to_act(&ppid, &ptid, proc);
                        }

                        if !rt.config.keep_processes() {
                            debug!(pid = %proc.id(), "remove process");
                            let cache = cache.clone();
                            let pid = proc.id().to_string();
                            cache.remove(&pid).unwrap_or_else(|err| {
                                error!(error = %err, "process remove failed");
                                false
                            });
                        }

                        let cache = cache.clone();
                        let rt = rt.clone();
                        cache
                            .restore(&rt)
                            .unwrap_or_else(|err| error!(error = %err, "process restore failed"));
                    }
                } else {
                    error!(pid = %proc.id(), "cannot find root task");
                }
            });
        }
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.emitter.on_task(move |e| {
                debug!(pid = %e.inner().pid, tid = %e.inner().id, "task event");
                let cache = cache.clone();
                let e_clone = e.clone();
                cache
                    .upsert_async(&e_clone)
                    .unwrap_or_else(|err| error!(error = %err, "task upsert failed"));

                // check task is allowed to emit message to client
                if !e.state().is_pending() && !e.state().is_running() && e.is_emit() {
                    let msg = e.create_message();
                    debug!(pid = %msg.pid, tid = %msg.tid, name = %msg.name, "emit message");
                    let emitter = rt.emitter().clone();
                    emitter.emit_message(&msg);
                }
            });
        }

        Ok(())
    }

    pub fn init_retry_timer(self: &Arc<Self>) -> crate::Result<()> {
        // Message retry timer — periodically re-send unacknowledged messages
        let max_message_retry_times = self.config().max_message_retry_times();
        #[cfg(not(test))]
        let interval_ms = {
            let secs = if self.config().tick_interval_secs() > 0 {
                self.config().tick_interval_secs()
            } else {
                15
            };
            (secs * 1000) as u64
        };
        #[cfg(test)]
        let interval_ms = 800u64;

        let evt = self.emitter().clone();
        let cache = self.cache.clone();
        let shutdown = self.shutdown.clone();
        Handle::current().spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _= shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
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

        Ok(())
    }

    fn return_to_act(self: &Arc<Self>, pid: &str, tid: &str, proc: &Process) {
        debug!(pid = %pid, tid = %tid, "return to act");
        let state = proc.state();
        // process.print();
        let mut vars = proc.outputs();
        debug!(pid = %pid, tid = %tid, outputs = %vars, "sub outputs");

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
            .map_err(|err| error!(error = %err, "return to act failed"));
    }
}
