mod act;
mod branch;
mod step;
mod workflow;

use crate::ActRunAs;
use crate::scheduler::{NextAction, Sign};
use crate::store::DbCollectionIden;
use crate::utils::consts::TASK_ROOT_TID;
use crate::{
    Act, ActError, ActTask, Error, Message, MessageState, NodeKind, Result, ShareLock, Variant,
    Vars, data,
    event::EventAction,
    scheduler::{
        Context, Process, PropagationPhase, Runtime, TaskState,
        tree::{Node, NodeContent},
    },
    utils::{self, consts},
};
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;
use std::sync::{
    Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, instrument};

#[derive(Clone)]
pub struct Task {
    /// process id
    pub pid: String,

    /// task id
    pub id: String,

    pub timestamp: i64,

    // task data
    data: ShareLock<Vars>,

    /// sealed data (read-only, written only by resolver)
    sealed_data: ShareLock<Vars>,

    /// the scope's vars row (data + sealed) diverged from the store since the
    /// last flush; set by every data mutation, cleared when the vars row is
    /// persisted (see `Cache::persist_task`)
    vars_dirty: Arc<AtomicBool>,

    /// bumped by every data mutation that dirties the scope — lets the
    /// persist path tell "no mutation happened while the vars row was being
    /// written" apart from "a mutation raced the write and must not be
    /// cleared", so a concurrent mutation can never be lost to a stale clear
    vars_gen: Arc<AtomicU64>,

    /// task state
    state: ShareLock<TaskState>,

    /// Fired when this task is overridden while it is running (see
    /// [`Task::cancelled`]); the act executing under it reads it through
    /// [`Context::cancelled`].
    cancel: CancellationToken,

    /// task error
    err: ShareLock<Option<Error>>,

    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    // previous tid
    prev: ShareLock<Option<String>>,

    // next tid
    next: ShareLock<Vec<String>>,

    // parent tid
    parent: ShareLock<Option<String>>,

    /// The owning process — held `Weak` so the process owns its task tree
    /// without a `Process → Task → Process` reference cycle: an evicted
    /// finished process is freed even though its tasks still reference it
    /// (see [`Self::proc`])
    proc: Weak<Process>,

    node: Arc<Node>,

    /// Serializes one task's action applications (see [`Self::enter_action`]):
    /// the guard check, the state write that decides the task and the walk the
    /// action triggers are one exclusive step per task, so two actions racing
    /// on the same task cannot both pass their guard and double-decide it.
    action_lock: Arc<tokio::sync::Mutex<()>>,

    runtime: Arc<Runtime>,
}

impl Task {
    pub fn new(proc: &Arc<Process>, tid: &str, node: Arc<Node>, rt: &Arc<Runtime>) -> Self {
        Self {
            pid: proc.id().to_string(),
            id: tid.to_string(),
            node,
            data: Arc::new(RwLock::new(Vars::new())),
            sealed_data: Arc::new(RwLock::new(Vars::new())),
            vars_dirty: Arc::new(AtomicBool::new(false)),
            vars_gen: Arc::new(AtomicU64::new(0)),
            state: Arc::new(RwLock::new(TaskState::None)),
            // a child of the runtime's shutdown token: this task's token fires
            // when the task is overridden *or* when the engine shuts down
            cancel: rt.shutdown_token().child_token(),
            err: Arc::new(RwLock::new(None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            prev: Arc::new(RwLock::new(None)),
            next: Arc::new(RwLock::new(Vec::new())),
            parent: Arc::new(RwLock::new(None)),
            timestamp: utils::time::timestamp(),
            proc: Arc::downgrade(proc),
            action_lock: Arc::new(tokio::sync::Mutex::new(())),
            runtime: rt.clone(),
        }
    }

    pub fn unique_id(&self) -> String {
        format!("{}:{}", self.pid, self.id)
    }

    /// The process this task belongs to, if it is still alive. The task holds
    /// its process only `Weak`ly (the process owns the task tree), so this is
    /// `None` only for a task clone that outlived its evicted, finished
    /// process — engine-driven paths always run against a live process.
    pub fn proc(&self) -> Option<Arc<Process>> {
        self.proc.upgrade()
    }

    /// The process for execution-time paths, which structurally require it to
    /// be alive (a context, message or the process itself cannot be built for
    /// a deallocated process).
    fn expect_proc(&self) -> Arc<Process> {
        self.proc.upgrade().unwrap_or_else(|| {
            panic!(
                "task '{}:{}' is used after its process was deallocated (evicted)",
                self.pid, self.id
            )
        })
    }

    pub(crate) fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Enter this task's action application — one at a time, in arrival order.
    ///
    /// [`Self::update`] holds the guard for the whole application, so an
    /// action's terminal guard, the state write that decides the task and the
    /// walk that write triggers (an abort's ancestor chain, a back's rewind)
    /// are exclusive against any other action on the same task. Without it the
    /// guards are check-then-apply and two racers (`Next` against `Abort`/
    /// `Back`, or a redelivery against the original) can both read a
    /// non-terminal state and both apply, leaving the task's state and the
    /// run's outcome decided twice.
    ///
    /// The second racer waits here rather than being turned away, then its own
    /// guard reads the decision the first one wrote — a genuine "already
    /// completed" refusal, and an action that failed before deciding does not
    /// swallow the one behind it. The lock is per task and never held while
    /// waiting on another task's application, so it cannot close a cycle.
    pub(crate) async fn enter_action(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.action_lock.clone().lock_owned().await
    }

    pub fn node(&self) -> &Arc<Node> {
        &self.node
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read()
    }

    pub fn state(&self) -> TaskState {
        let state = &*self.state.read();
        state.clone()
    }

    /// A token that fires once this task was overridden while it was running
    /// (see [`Task::set_state`]) or the engine began shutting down — the task's
    /// token is a child of the runtime's shutdown token. Acts read it through
    /// [`Context::cancellation_token`].
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }
        utils::time::time_millis() - self.start_time()
    }

    pub fn is_emit(&self) -> bool {
        let Some(v) = self.sign() else {
            return true;
        };

        let is_no_emit = (v & Sign::NO_EMIT) == Sign::NO_EMIT;
        !is_no_emit
    }

    pub fn set_emit(&self, v: bool) {
        if v {
            self.remove_sign(Sign::NO_EMIT);
        } else {
            self.set_sign(Sign::NO_EMIT);
        }
    }

    pub fn is_auto_complete(&self) -> bool {
        let Some(v) = self.sign() else {
            return true;
        };

        let is_no_automate = (v & Sign::NO_AUTO_COMPLETE) == Sign::NO_AUTO_COMPLETE;
        !is_no_automate
    }

    pub fn is_sign(&self, sign: Sign) -> bool {
        self.with_data(|data| {
            if let Some(ref v) = data.get::<Sign>(consts::TASK_SIGN) {
                return (*v & sign) == sign;
            }
            false
        })
    }

    pub fn sign(&self) -> Option<Sign> {
        self.with_data(|data| data.get::<Sign>(consts::TASK_SIGN))
    }

    /// The durable phase of propagating this task's outcome to its parent.
    /// Unlike the in-memory [`NextAction`], this is persisted with the task's
    /// vars row and therefore survives a crash.
    pub fn propagation_phase(&self) -> PropagationPhase {
        self.with_data(|data| {
            data.get::<String>(PropagationPhase::task_key())
                .as_deref()
                .map_or(PropagationPhase::None, |value| {
                    PropagationPhase::from_task_value(Some(value))
                })
        })
    }

    pub fn set_propagation_phase(&self, phase: PropagationPhase) {
        let key = PropagationPhase::task_key();
        self.set_data_with(move |data| match phase.as_task_value() {
            Some(value) => data.set(key, value),
            None => {
                data.remove(key);
            }
        });
    }

    /// A durable propagation guard: once applied, replaying the same
    /// propagation must not re-schedule children, re-merge outputs, or move an
    /// already-processed terminal outcome to the parent again.
    pub fn is_propagation_applied(&self) -> bool {
        self.propagation_phase().is_applied()
    }

    /// Whether this task's propagation is *finished*, and a replay of its
    /// `next` may be treated as a no-op that only closes its outbox record.
    ///
    /// The applied marker alone is not that proof. It lives in the scope's
    /// vars row while the lifecycle row carrying the terminal state is written
    /// by the task's own transitions, and the two are separate store writes; a
    /// write cut (a crash) between them therefore leaves `applied` beside a
    /// non-terminal state — a pair in which the propagation genuinely never
    /// finished (the parent was never completed), so recovery must replay it
    /// instead of closing it as done. Both halves together are the guard.
    pub fn is_propagation_done(&self) -> bool {
        self.state().is_completed() && self.is_propagation_applied()
    }

    /// Whether this task's own `next` pass still has work that in-memory
    /// progress can do — i.e. re-driving it can only move the run forward:
    ///
    /// - **running and auto-completing**: the pass completes the task from its
    ///   children, which is what carries a finished subtree to its parent.
    ///
    /// Everything else is left to the path that already owns it. A `next`
    /// record of a *terminal* task is **closed** by the outbox pass, never
    /// replayed: a re-run would schedule from a state a decision walk (cancel,
    /// back, remove, abort) or an interrupted act already resolved, and a
    /// decision's work is not the scheduler's to redo. A task that is not
    /// `Running` and not terminal waits on the outside world by design — an
    /// `Interrupt` for a client action, a `Pending` for its sibling branches,
    /// a `None`/`Ready` for its first run, a subflow act (`NO_AUTO_COMPLETE`)
    /// for the child process it started — and its open record is the contract,
    /// not a stall.
    pub(crate) fn next_is_drivable(&self) -> bool {
        self.state().is_running() && self.is_auto_complete()
    }

    pub fn set_sign(&self, sign: Sign) {
        self.set_data_with(move |data| {
            if let Some(ref v) = data.get::<Sign>(consts::TASK_SIGN) {
                data.set(consts::TASK_SIGN, *v | sign);
            } else {
                data.set(consts::TASK_SIGN, sign);
            }
        });
    }

    pub fn remove_sign(&self, sign: Sign) {
        self.set_data_with(move |data| {
            if let Some(ref v) = data.get::<Sign>(consts::TASK_SIGN) {
                data.set(consts::TASK_SIGN, *v & !sign);
            }
        });
    }
    pub fn set_auto_complete(&self, v: bool) {
        if v {
            self.remove_sign(Sign::NO_AUTO_COMPLETE);
        } else {
            self.set_sign(Sign::NO_AUTO_COMPLETE);
        }
    }
    pub fn create_context(self: &Arc<Self>) -> Context {
        self.expect_proc().create_context(self)
    }

    pub fn create_message(self: &Arc<Self>) -> Message {
        let workflow = self.expect_proc().model();
        // if it is act, insert the step_node_id and step_task_id to the inputs
        // it is necessary to find the relation between the step and it's children acts
        let mut inputs = self.inputs();
        if self.node.kind() == NodeKind::Act {
            let mut parent = self.parent();
            while let Some(task) = parent {
                if task.is_kind(NodeKind::Step) {
                    inputs.insert(
                        consts::STEP_KEY.to_string(),
                        json!({
                            consts::STEP_NODE_ID: task.node.id(),
                            consts::STEP_NODE_NAME: task.node.name(),
                            consts::STEP_TASK_ID: task.id,
                        }),
                    );
                    break;
                }
                parent = task.parent();
            }

            // append act.params to inputs
            inputs.set(consts::ACT_PARAMS_KEY, self.params());
        }

        // append act.optins to inputs
        inputs.set(consts::ACT_OPTIONS_KEY, self.options());

        // append workflow model to inputs
        inputs.set(
            consts::WORKFLOW_MODEL_KEY,
            Vars::new()
                .with("id", &workflow.id)
                .with("name", &workflow.name)
                .with("options", &workflow.options),
        );

        // add error to inputs
        if let Some(err) = self.err() {
            inputs.set(consts::ACT_ERR_CODE, err.ecode);
            inputs.set(consts::ACT_ERR_MESSAGE, err.message);
        }

        let state: MessageState = self.state().into();
        Message {
            id: utils::longid(),
            delivery_id: None,
            tid: self.id.clone(),
            name: self.node.content.name(),
            r#type: self.node.kind().to_string(),
            state,
            pid: self.pid.clone(),
            nid: self.node.id().to_string(),
            mid: workflow.id.clone(),
            uses: self.node.uses(),
            inputs,
            outputs: self.outputs(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            retry_times: 0,
            timestamp: self.timestamp,
        }
    }

    pub fn prev_id(&self) -> Option<String> {
        let ret = self.prev.read();
        ret.clone()
    }

    pub fn next_ids(&self) -> Vec<String> {
        let ret = self.next.read();
        ret.clone()
    }

    pub fn parent_id(&self) -> Option<String> {
        let ret = self.parent.read();
        ret.clone()
    }

    pub fn parent(&self) -> Option<Arc<Task>> {
        let parent = self.parent.read().clone()?;
        self.proc()?.task(&parent)
    }

    pub fn children(&self) -> Vec<Arc<Self>> {
        self.proc()
            .map(|proc| proc.children(&self.id))
            .unwrap_or_default()
    }

    pub fn next(&self) -> Vec<Arc<Self>> {
        let Some(proc) = self.proc() else {
            return Vec::new();
        };
        let mut ret = Vec::new();
        let nexts = self.next_ids();
        for tid in &nexts {
            if let Some(task) = proc.task(tid) {
                ret.push(task);
            }
        }
        ret
    }

    pub fn siblings(&self) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        if let Some(parent) = self.parent() {
            let children = parent.children();
            ret.extend(children.iter().filter(|iter| iter.id != self.id).cloned());
        }

        ret
    }

    pub fn inputs(self: &Arc<Self>) -> Vars {
        let ctx = self.create_context();
        let mut inputs = Vars::new();
        if let Some(prev) = self.prev_id()
            && let Some(prev_task) = self.proc().and_then(|proc| proc.task(&prev))
        {
            // set the prev task's outputs as current inputs
            for (ref k, v) in &prev_task.outputs() {
                inputs.set(k, v.clone());
            }
        }
        // merge the node vars
        let vars = utils::fill_inputs(&self.node.content.vars(), &ctx);
        inputs.extend(vars)
    }

    pub fn outputs(self: &Arc<Self>) -> Vars {
        let ctx = self.create_context();
        let mut outputs = Vars::new();
        let mut exposes = self.node.content.exposes().clone();
        if exposes.is_empty() {
            // fallback: check options for exposes (runtime push actions)
            if let Some(opt_exposes) = self.options().get::<Vec<Variant>>("exposes") {
                exposes = opt_exposes;
            }
        }
        if !exposes.is_empty() {
            for var in &exposes {
                outputs.set(&var.name, var.value.clone());
            }
        } else {
            // export all data except the private ones
            for (key, _) in ctx.task().data().iter() {
                if !consts::is_private_key(key) {
                    outputs.set(key, json!(null))
                }
            }
        }

        utils::fill_outputs(&outputs, &ctx)
    }

    pub fn options(self: &Arc<Self>) -> Vars {
        self.node.content.options()
    }

    pub fn params(self: &Arc<Self>) -> serde_json::Value {
        let ctx = self.create_context();
        utils::fill_params(&self.node.content.params(), &ctx)
    }

    pub fn set_prev(&self, prev: &str) {
        *self.prev.write() = Some(prev.to_string());
    }

    pub fn set_parent(&self, parent: &str) {
        *self.parent.write() = Some(parent.to_string());
    }

    pub fn set_next(&self, next: &str) {
        self.next.write().push(next.to_string());
    }

    pub fn set_state(&self, state: TaskState) {
        self.set_state_impl(state, false);
    }

    /// Move the task to `state` only while it is still `Running`, as one
    /// atomic step: a decision that landed meanwhile — a client `abort`, an
    /// error, or the terminal walk of the task's own subtree — must stick
    /// instead of being overwritten by a completion pass that read the state
    /// before it changed. Returns whether the transition happened.
    pub fn set_state_if_running(&self, state: TaskState) -> bool {
        self.set_state_impl(state, true)
    }

    fn set_state_impl(&self, state: TaskState, only_if_running: bool) -> bool {
        // The check and the write share one lock: a `is_running()` read
        // followed by a write is exactly the window a racing decision slips
        // through.
        let mut cur = self.state.write();
        if only_if_running && !cur.is_running() {
            return false;
        }
        // An override of a running task (an `abort`/`cancel`/`skip`/`next`/
        // `remove` action, or an error) must stop the work the task started.
        // The token reaches the act's own execution through
        // [`Context::cancelled`], so the act gives up its child process,
        // request or subscription instead of holding its scheduler lane until
        // it finishes on its own.
        //
        // Only a running task is overridden: a task that already reached a
        // terminal state is never dispatched again (a redo builds a new task),
        // so the token is never observed by a later run of this instance.
        if cur.is_running() && state.is_completed() {
            self.cancel.cancel();
        }

        if state.is_completed() {
            self.set_end_time(utils::time::time_millis());

            if self.id == TASK_ROOT_TID
                && let Some(proc) = self.proc()
            {
                proc.set_state(state.clone());
            }
        } else {
            // re-entering a non-terminal state: reset the propagation guard
            self.set_propagation_phase(PropagationPhase::None);
            if state.is_created() {
                self.set_start_time(utils::time::time_millis());
            }
        }
        *cur = state.clone();
        drop(cur);

        // clean the err
        if state != TaskState::Error {
            *self.err.write() = None;
        }
        true
    }

    pub fn set_err(self: &Arc<Self>, err: &Error) {
        *self.err.write() = Some(err.clone());

        // An errored scope carries its own data only (see [`Self::update_data`]),
        // so the local overlay a failed run left behind is refreshed with the
        // data it started from — its **inputs** ([`Self::inputs`]: the
        // predecessor's outputs plus this task's node vars) — before the error
        // fields land. The parent scope is one level above this task and is not
        // what seeded it, so it is not the source here; the input values are
        // written onto this scope's data.
        self.set_data(&self.inputs());

        self.set_data_with(|data| {
            data.set(consts::ACT_ERR_CODE, &err.ecode);
            data.set(consts::ACT_ERR_MESSAGE, &err.message)
        });
        self.set_state(TaskState::Error);
    }

    pub fn clear_err_with(&self, new_state: TaskState) {
        *self.err.write() = None;
        self.set_data_with(|data| {
            data.remove(consts::ACT_ERR_CODE);
            data.remove(consts::ACT_ERR_MESSAGE);
        });
        self.set_state(new_state);
    }

    pub(crate) fn set_pure_err(&self, err: &Error) {
        *self.err.write() = Some(err.clone());
    }

    pub fn err(&self) -> Option<Error> {
        self.err.read().clone()
    }

    pub fn set_pure_state(&self, state: TaskState) {
        *self.state.write() = state;
    }

    pub fn set_start_time(&self, time: i64) {
        *self.start_time.write() = time;
    }
    pub fn set_end_time(&self, time: i64) {
        *self.end_time.write() = time;
    }

    pub fn is_kind(&self, kind: NodeKind) -> bool {
        self.node.kind() == kind
    }

    pub fn is_uses(&self, v: &str) -> bool {
        if self.node.kind() == NodeKind::Act {
            return self.node.uses().as_deref() == Some(v);
        }
        false
    }

    pub fn is_timeouts(&self) -> bool {
        match &self.node.content {
            NodeContent::Step(step) => !step.timeouts.is_empty(),
            _ => false,
        }
    }

    /// The task that timed out, when this task is one of its step's declared
    /// `timeouts` branches. That task owns the branch's one-shot marker (see
    /// [`Self::claim_timeout`]), and it is also the key the tick dispatches a
    /// branch under ([`Self::on_timeout`](crate::ActTask::on_timeout) in
    /// `Step`), because a branch is a partial projection of that declaration.
    pub fn timeout_owner(&self) -> Option<Arc<Task>> {
        let parent = self.parent()?;
        match &parent.node().content {
            NodeContent::Step(step) => step
                .timeouts
                .iter()
                .any(|branch| branch.id == self.node().id())
                .then_some(parent),
            _ => None,
        }
    }

    /// Whether the timeout branch `node_id` already fired for this task (see
    /// [`Self::claim_timeout`]).
    pub fn is_timeout_claimed(&self, node_id: &str) -> bool {
        self.with_data(|data| {
            data.get::<Vec<String>>(consts::TASK_TIMEOUTS)
                .is_some_and(|fired| fired.iter().any(|id| id == node_id))
        })
    }

    /// Claim the one-shot timeout slot of `node_id` on this task. Returns
    /// `true` when this call set the marker — the caller then dispatches the
    /// branch — and `false` when that branch already fired.
    ///
    /// The marker is part of the task's own scope data, so it is persisted
    /// with the task's vars row (see [`Self::release_timeout`] for the
    /// not-made-durable case) and a restored process does not re-fire a
    /// branch that already fired. The check and the set happen under the
    /// scope lock, so two ticks cannot both claim one slot; distinct branches
    /// hold distinct slots.
    pub fn claim_timeout(&self, node_id: &str) -> bool {
        let mut data = self.data.write();
        let mut fired: Vec<String> = data.get(consts::TASK_TIMEOUTS).unwrap_or_default();
        if fired.iter().any(|id| id == node_id) {
            return false;
        }
        fired.push(node_id.to_string());
        data.set(consts::TASK_TIMEOUTS, fired);
        // the generation is bumped before the dirty flag, so a persist running
        // concurrently cannot clear this mutation away
        self.mark_vars_dirty();
        true
    }

    /// Release a claim taken by [`Self::claim_timeout`] — used when the claim
    /// could not be made durable, so the branch stays eligible on the next
    /// tick instead of being silently consumed.
    pub fn release_timeout(&self, node_id: &str) {
        let mut data = self.data.write();
        let Some(mut fired) = data.get::<Vec<String>>(consts::TASK_TIMEOUTS) else {
            return;
        };
        let before = fired.len();
        fired.retain(|id| id != node_id);
        if fired.len() == before {
            return;
        }
        if fired.is_empty() {
            data.pop::<Vec<String>>(consts::TASK_TIMEOUTS);
        } else {
            data.set(consts::TASK_TIMEOUTS, fired);
        }
        self.mark_vars_dirty();
    }

    pub fn is_catches(&self) -> bool {
        match &self.node.content {
            NodeContent::Step(step) => !step.catches.is_empty(),
            _ => false,
        }
    }

    #[instrument(skip(self, ctx), fields(pid = %self.pid, tid = %self.id))]
    pub async fn exec(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        // let _lock = self.sync.lock().unwrap();
        debug!(kind = %self.node().kind(), name = %self.node().name(), uses = ?self.node().uses(), "task started");
        if self.state().is_completed() {
            return Err(ActError::Runtime(format!(
                "task({}:{}) is already completed",
                self.pid, self.id
            )));
        }
        self.init(ctx).await?;
        self.run(ctx).await?;
        ctx.push_next().await?;
        Ok(())
    }

    #[instrument(skip(self, ctx), fields(pid = %self.pid, tid = %self.id))]
    pub async fn update(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        // One action at a time decides this task. Everything below — the
        // terminal guards, the state write the winning arm makes and the walk
        // it runs — happens under this task's claim, so an action racing it
        // (another client event on the same tid, a redelivered one) waits and
        // then reads the decision instead of passing the same guard.
        let _action = self.enter_action().await;
        debug!("task updated");
        let action = ctx.action().ok_or(ActError::Action(
            "cannot find action in context".to_string(),
        ))?;
        // helpers (e.g. `abort_task`) may re-point `ctx.task()` while applying
        // the action, so capture the action's own task for the outbox close
        let action_task = ctx.task().clone();

        // durable action outbox: non-`Next` client events are recorded before
        // applying so a crash before the state write lands can be replayed on
        // recovery; `Next` uses its own outbox (`push_next`), `Push` is
        // internal
        let action_outbox = !matches!(&action.event, EventAction::Next | EventAction::Push);
        if action_outbox {
            ctx.runtime.enqueue_action(&action).await?;
            if let Err(err) = ctx
                .runtime
                .cache()
                .mark_op_phase(
                    &action.pid,
                    &action.tid,
                    crate::data::OpType::Action,
                    crate::data::OpPhase::EffectInFlight,
                )
                .await
            {
                error!(error = %err, "failed to mark action effect-in-flight");
            }
        }

        let result: Result<()> = (async {
            match &action.event {
                EventAction::Push => {
                    let package = ctx.get_var::<String>("uses").unwrap_or_default();
                    let act = Act {
                        id: ctx.get_var::<String>("id").unwrap_or_default(),
                        name: ctx.get_var::<String>("name").unwrap_or_default(),
                        desc: ctx.get_var::<String>("desc").unwrap_or_default(),
                        r#if: ctx.get_var::<String>("if"),
                        vars: ctx.get_var::<Vec<Variant>>("vars").unwrap_or_default(),
                        uses: package.clone(),
                        params: ctx.get_var("params").unwrap_or_default(),
                        options: ctx.get_var("options").unwrap_or_default(),
                        exposes: ctx.get_var("exposes").unwrap_or_default(),
                        ..Default::default()
                    };

                    // check key property
                    if package.is_empty() {
                        return Err(crate::ActError::Action(
                            "cannot find 'uses' in act".to_string(),
                        ));
                    }

                    ctx.dispatch_act(&act, Vars::new())?;
                }
                EventAction::Remove => {
                    self.set_state(TaskState::Removed);
                    ctx.emit_task(self).await?;
                    ctx.push_next().await?;
                }
                EventAction::Submit => {
                    self.update_data(&ctx.vars());
                    self.set_state(TaskState::Submitted);
                    ctx.emit_task(self).await?;
                    ctx.push_next().await?;
                }
                EventAction::Next => {
                    if self.state().is_completed() {
                        return Err(ActError::Action(format!(
                            "task '{}:{}' is already completed",
                            self.pid, self.id
                        )));
                    }
                    self.update_data(&ctx.vars());
                    self.set_state(TaskState::Completed);
                    ctx.emit_task(self).await?;
                    ctx.push_next().await?;
                }
                EventAction::Back => {
                    if self.state().is_completed() {
                        return Err(ActError::Action(format!(
                            "task '{}:{}' is already completed",
                            self.pid, self.id
                        )));
                    }
                    let nid = ctx
                        .get_var::<String>(consts::ACT_TO)
                        .ok_or(ActError::Action(
                            "cannot find 'to' value in options".to_string(),
                        ))?;

                    let mut path_tasks = Vec::new();
                    let task = self.backs(
                        &|t| t.node.kind() == NodeKind::Step && t.node.id() == nid,
                        &mut path_tasks,
                    );

                    let task = task.ok_or(ActError::Action(format!(
                        "cannot find history task by nid '{nid}'",
                    )))?;

                    // Register the replacement task BEFORE the rewind marks the
                    // history path terminal. `back_task` leaves every step of
                    // the path `Completed`/`Backed`; should a concurrently
                    // re-dispatched `next` pass (recovery replays them) observe
                    // the process with all children terminal in between, it
                    // would complete the whole workflow and the redo task would
                    // be an orphan under a terminal process. `redo_task` is
                    // synchronous, so the new child is visible to any such pass
                    // the moment this returns.
                    ctx.redo_task(&task)?;
                    ctx.back_task(&ctx.task(), &path_tasks).await?;
                }
                EventAction::Cancel => {
                    // find the parent step task
                    let mut step = ctx.task().parent();
                    while let Some(task) = &step {
                        if task.is_kind(NodeKind::Step) {
                            break;
                        }
                        step = task.parent();
                    }

                    let task = step.ok_or(ActError::Action(format!(
                        "cannot find parent step task by tid '{}'",
                        ctx.task().id,
                    )))?;
                    if !task.state().is_biz_success() {
                        return Err(ActError::Action(format!(
                            "task('{}') is not allowed to cancel",
                            task.id
                        )));
                    }

                    // get the neartest next step tasks
                    let mut path_tasks = Vec::new();
                    let nexts = task.follows(
                        &|t| t.is_kind(NodeKind::Step) && t.is_acts(),
                        &mut path_tasks,
                    );
                    if nexts.is_empty() {
                        return Err(ActError::Action("cannot find cancelled tasks".to_string()));
                    }

                    // A cancel rewinds forward work. When that work is already
                    // terminal the cancel was already applied — a duplicate
                    // delivery or a replayed durable record — and rewinding
                    // again would create a second redo path the client never
                    // drives: an orphan step/act that waits forever and wedges
                    // the run. Refuse before any redo is created.
                    if nexts.iter().all(|n| n.state().is_completed()) {
                        return Err(ActError::Action(format!(
                            "task('{}') is not allowed to cancel",
                            task.id
                        )));
                    }

                    // Register the replacement task BEFORE the undo marks the
                    // cancelled path terminal — same hazard as `Back`: between
                    // the terminal states and the redo task, a concurrently
                    // re-dispatched `next` pass would see every child of the
                    // process terminal and complete the workflow. `redo_task` is
                    // synchronous, so the new child exists before the first
                    // `emit_task` await below.
                    ctx.redo_task(&task)?;

                    // mark the path tasks as completed
                    for p in path_tasks {
                        if p.state().is_running() {
                            p.set_state(TaskState::Completed);
                            ctx.emit_task(&p).await?;
                            ctx.close_decided_propagation(&p).await;
                        } else if p.state().is_pending() {
                            p.set_state(TaskState::Skipped);
                            ctx.emit_task(&p).await?;
                            ctx.close_decided_propagation(&p).await;
                        }
                    }

                    for next in &nexts {
                        ctx.undo_task(next).await?;
                    }
                }
                EventAction::Abort => {
                    if self.state().is_completed() {
                        return Err(ActError::Action(format!(
                            "task '{}:{}' is already completed",
                            self.pid, self.id
                        )));
                    }
                    ctx.abort_task(&ctx.task()).await?;
                }
                EventAction::Skip => {
                    if self.state().is_completed() {
                        return Err(ActError::Action(format!(
                            "task '{}:{}' is already completed",
                            self.pid, self.id
                        )));
                    }

                    for task in self.siblings() {
                        if task.state().is_completed() {
                            continue;
                        }
                        task.set_state(TaskState::Skipped);
                        ctx.emit_task(&task).await?;
                        ctx.close_decided_propagation(&task).await;
                    }

                    // set both current act and parent step to skip
                    self.set_state(TaskState::Skipped);
                    ctx.emit_task(self).await?;
                    ctx.push_next().await?;
                }
                EventAction::Error => {
                    let ecode =
                        ctx.get_var::<String>(consts::ACT_ERR_CODE)
                            .ok_or(ActError::Action(format!(
                                "cannot find '{}' in options",
                                consts::ACT_ERR_CODE
                            )))?;

                    let error = ctx
                        .get_var::<String>(consts::ACT_ERR_MESSAGE)
                        .unwrap_or("".to_string());

                    let err = Error::new(&error, &ecode);
                    debug!(error = ?err, "task error");
                    let task = &ctx.task();
                    if task.state().is_completed() {
                        return Err(ActError::Action(format!(
                            "task '{}:{}' is already completed",
                            task.pid, task.id
                        )));
                    }
                    let parent = task.parent().ok_or(ActError::Action(format!(
                        "cannot find task parent by tid '{}'",
                        task.id
                    )))?;

                    for sub in parent.siblings().iter() {
                        if sub.state().is_completed() {
                            continue;
                        }
                        sub.set_state(TaskState::Skipped);
                        ctx.emit_task(sub).await?;
                        ctx.close_decided_propagation(sub).await;
                    }
                    task.set_err(&err);
                    task.set_data(&ctx.vars());
                    task.on_error(ctx).await?;
                    // The error walk queued this act's state write; a transient
                    // store fault there would leave the act durably waiting
                    // (`interrupt`) while the client was already told it
                    // errored — a crash would then restore an act nobody will
                    // ever complete. Re-emit the task so the write is queued
                    // again and the durable row converges to `error` as soon as
                    // the backend does.
                    ctx.emit_task(task).await?;
                }
                EventAction::SetProcessVars => {
                    if self.state().is_completed() {
                        return Err(ActError::Action(format!(
                            "task '{}:{}' is already completed",
                            self.pid, self.id
                        )));
                    }

                    let proc = self.expect_proc();
                    proc.set_data(&ctx.vars());
                    // The root scope's row is written by the root's own persist
                    // (no descendant's persist walks up to it any more), so
                    // queue it here: FIFO keeps the vars row durable before the
                    // action's outbox record closes, so an acknowledged
                    // `set_process_var` survives a cut that lands before the
                    // root's next event.
                    if let Some(root) = proc.root() {
                        ctx.runtime.cache().upsert_async(&root).await?;
                    }
                    // emit the task change (issue #)
                    ctx.emit_task(self).await?;
                }
            }
            Ok(())
        })
        .await;

        if result.is_ok() && action.event != EventAction::Push {
            // close the task's deliveries after doing the action (deferred to
            // the writer thread); an `Error` delivery stays for manual handling
            ctx.runtime
                .cache()
                .close_deliveries(&action.pid, &action.tid)
                .await?;
        }

        if action_outbox {
            // close the action's outbox record: the state write (emit_task) and
            // the message status were already queued above, so FIFO order makes
            // `Done` durable only after both. An errored application is closed
            // too — nothing to replay.
            if let Err(err) = ctx.runtime.complete_action(&action_task).await {
                error!(error = %err, "complete_action failed");
            }
        }

        result
    }

    pub fn is_ready(&self) -> bool {
        match &self.node.content {
            NodeContent::Branch(n) => {
                let siblings = self.siblings();
                if !n.needs.is_empty() {
                    if siblings
                        .iter()
                        .filter(|iter| {
                            iter.state().is_completed()
                                && n.needs.contains(&iter.node.id().to_string())
                        })
                        .count()
                        > 0
                    {
                        return true;
                    }
                    return false;
                }

                if n.r#else {
                    if siblings.iter().all(|iter| iter.state().is_skip()) {
                        return true;
                    }

                    // fix the branch.default state
                    if siblings.iter().any(|iter| {
                        iter.state().is_error()
                            || iter.state().is_biz_success()
                            || iter.state().is_abort()
                    }) {
                        self.set_state(TaskState::Skipped);
                    }
                }

                false
            }
            _ => true,
        }
    }

    pub async fn resume(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        if self.is_ready() {
            self.set_state(TaskState::Running);
            ctx.runtime.emitter().emit_task_event(self).await?;
            self.exec(ctx).await?;
        }

        Ok(())
    }

    pub fn into_data(self: &Arc<Self>) -> Result<data::Task> {
        let id = utils::Id::new(&self.pid, &self.id);
        Ok(data::Task {
            id: id.id(),
            prev: self.prev_id(),
            next: self.next_ids(),
            parent: self.parent_id(),
            name: self.node.content.name(),
            kind: self.node.kind().to_string(),
            pid: self.pid.clone(),
            tid: self.id.clone(),
            node_data: self.node.to_string()?,
            state: self.state().into(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            timestamp: self.timestamp,
            err: self.err().map(|err| err.to_string()),
            v: data::Task::version(),
        })
    }

    /// The scope vars row paired with this task's lifecycle row: the task's
    /// own `data` and `sealed`, stored apart from the lifecycle row.
    pub fn into_data_vars(self: &Arc<Self>) -> Result<data::TaskVars> {
        let id = utils::Id::new(&self.pid, &self.id);
        Ok(data::TaskVars {
            id: id.id(),
            pid: self.pid.clone(),
            tid: self.id.clone(),
            data: self.data().to_string(),
            sealed: self.sealed_data.read().to_string(),
            v: data::TaskVars::version(),
        })
    }

    /// check if the task includes act
    fn is_acts(&self) -> bool {
        self.children()
            .iter()
            .any(|iter| iter.is_kind(NodeKind::Act))
    }

    fn backs<F: Fn(&Arc<Self>) -> bool + Clone>(
        &self,
        predicate: &F,
        path: &mut Vec<Arc<Self>>,
    ) -> Option<Arc<Self>> {
        let mut ret = None;

        let mut prev = self.prev_id();
        while let Some(tid) = &prev {
            if let Some(task) = self.proc().and_then(|proc| proc.task(tid)) {
                if predicate(&task) {
                    ret = Some(task.clone());
                    break;
                }

                // push the path tasks
                if task.state().is_running() || task.state().is_pending() {
                    path.push(task.clone());
                }

                prev = task.prev_id();
            } else {
                prev = None
            }
        }

        ret
    }

    fn follows<F: Fn(&Arc<Self>) -> bool + Clone>(
        &self,
        predicate: &F,
        path: &mut Vec<Arc<Self>>,
    ) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        let nexts = self.next();
        if !nexts.is_empty() {
            for task in &nexts {
                if predicate(task) {
                    ret.push(task.clone());
                } else {
                    // push the path tasks
                    if task.state().is_running() || task.state().is_pending() {
                        path.push(task.clone());
                    }

                    // find the next follows
                    ret.extend(task.follows(predicate, path));
                }
            }
        }

        ret
    }

    pub fn is_next(&self) -> bool {
        let state = self.state();
        state.is_completed() || state.is_interrupted()
    }

    pub async fn check_uses_action(&self, ctx: &Context) -> Result<NextAction> {
        let task = ctx.task();
        if task.state().is_running()
            && task.node().kind() == NodeKind::Step
            && task.node().uses().is_some()
            && !self.is_sign(Sign::USES_COMPLETE)
        {
            let mut count = 0;
            let task_children = self.children();
            let task_children = task_children
                .iter()
                .filter(|t| t.node().kind() == NodeKind::Act)
                .collect::<Vec<_>>();

            for task in task_children.iter() {
                if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Ready);
                    self.runtime.emitter().emit_task_event(task).await?;
                    task.exec(ctx).await?;
                }
                // A child only counts once its own `next` has run
                // (a finished propagation), because that is what propagates
                // the child's outputs into this task. A terminal state alone is
                // written by whatever job applied the child's action — e.g.
                // `acts.core.action` setting `Submitted` inside the `exec`
                // above — and the child's queued `next` may still be behind
                // this job, so counting the state would complete this step
                // (and the whole workflow) with the child's outputs missing.
                // The child's `next` re-enters this step's `next`, which then
                // observes the marker and proceeds. The marker alone is not
                // enough either: a cut write can leave it durable beside a
                // non-terminal state (see [`Task::is_propagation_done`]).
                if task.is_propagation_done() {
                    count += 1;
                }
            }

            if count != task_children.len() {
                return Ok(NextAction::Stop);
            }

            // marked sign flag when all children task completed
            self.set_sign(Sign::USES_COMPLETE);
        }

        Ok(NextAction::Continue)
    }

    pub async fn check_in_children(self: &Arc<Self>, ctx: &Context) -> Result<NextAction> {
        if self.state().is_running() {
            // run into children nodes if there is children nodes
            // The marker is this pass's in-memory fast path. It is not what
            // makes the pass crash-safe: it lives in the scope's vars row,
            // which no persist need reach while this task runs on (its own
            // `next` record stays `Pending` by design while its children are in
            // flight), so recovery may replay the visit without it. What makes
            // the replay a no-op is the slot itself — see
            // [`Context::schedule_visited`].
            if !self.is_sign(Sign::IN_CHILDREN) {
                let children = self.node().children();
                if !children.is_empty() {
                    let mut scheduled = false;
                    for child in &children {
                        scheduled |= ctx.schedule_visited(child, ctx.task())?;
                    }
                    self.set_sign(Sign::IN_CHILDREN);
                    // A replay that found every child already scheduled has
                    // nothing to wait for: it must leave the pass where a
                    // later one would sit — auto-completing this task when its
                    // children are all done — instead of stopping on a
                    // one-shot visit it did not perform. Any child it *did*
                    // schedule has to run first, so the visit still stops.
                    if scheduled {
                        return Ok(NextAction::Stop);
                    }
                }
            }
        }

        Ok(NextAction::Continue)
    }

    pub async fn auto_complete(self: &Arc<Self>, ctx: &Context) -> Result<NextAction> {
        let state = self.state();

        if state.is_running() {
            let task_children = self.children();
            let mut count = 0;

            // for msg act, the client can only receive 'completed' message
            if self.node().kind() == NodeKind::Act
                && let Some(run_as) = ctx
                    .task()
                    .with_data(|data| data.get::<ActRunAs>(consts::ACT_RUN_AS))
                && run_as == ActRunAs::Msg
            {
                self.set_state(TaskState::Completed);
            }

            for task in task_children.iter() {
                if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Ready);
                    self.runtime.emitter().emit_task_event(task).await?;
                    task.exec(ctx).await?;
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            // A child the abort walk stopped is not a business success: that
            // walk owns this task's outcome and marks the ancestors `Aborted`
            // itself. It sets the target `Aborted` first and only then moves
            // up, so completing here in between would re-complete a step the
            // walk has not reached yet as `Completed` and let it schedule its
            // chain under a run the walk is ending. The other terminal states
            // stay completable: a skipped/cancelled/removed child is a
            // business decision, not a stopped subtree.
            let aborted = task_children.iter().any(|t| t.state().is_abort());

            if !aborted
                && count == task_children.len()
                && self.is_auto_complete()
                && !self.state().is_completed()
                && !self
                    .runtime
                    .cache()
                    .store()
                    .has_pending_propagation(&self.pid, &self.id)
                    .await?
            {
                // check if the task is error catched
                let is_empty_catched = task_children
                    .iter()
                    .filter(|t| t.is_sign(Sign::CATCH))
                    .all(|t| t.state().is_skip());

                if self.is_sign(Sign::ERROR) && is_empty_catched {
                    // no any action to match
                    // resume the task error state
                    let err = self.with_data(|data| {
                        Error::new(
                            &data
                                .get::<String>(consts::ACT_ERR_MESSAGE)
                                .unwrap_or_default(),
                            &data.get::<String>(consts::ACT_ERR_CODE).unwrap_or_default(),
                        )
                    });
                    self.set_err(&err);
                    ctx.emit_error().await?;
                    return Ok(NextAction::Stop);
                } else {
                    // only complete a task that is still running: a decision
                    // that landed meanwhile (abort/error) must stick
                    self.set_state_if_running(TaskState::Completed);
                }
            }

            if self.state().is_completed() {
                ctx.emit_task(self).await?;
            }
        }

        // continue to run next
        Ok(NextAction::Continue)
    }
}

impl ActTask for Arc<Task> {
    #[instrument(skip(self, ctx), fields(pid = %self.pid, tid = %self.id))]
    async fn init(&self, ctx: &Context) -> Result<()> {
        debug!("task init");
        ctx.set_task(self);
        if ctx.task().state().is_none() {
            ctx.prepare().await?;
            ctx.task().set_state(TaskState::Ready);
            match &self.node.content {
                NodeContent::Workflow(workflow) => workflow.init(ctx).await?,
                NodeContent::Branch(branch) => branch.init(ctx).await?,
                NodeContent::Step(step) => step.init(ctx).await?,
                NodeContent::Act(act) => act.init(ctx).await?,
            }
            ctx.emit_task(&ctx.task()).await?;
        }

        Ok(())
    }

    #[instrument(skip(self, ctx), fields(pid = %self.pid, tid = %self.id))]
    async fn run(&self, ctx: &Context) -> Result<()> {
        debug!("task running");
        let task = ctx.task();
        if task.state().is_ready() {
            task.set_state(TaskState::Running);
            match &self.node.content {
                NodeContent::Workflow(workflow) => workflow.run(ctx).await,
                NodeContent::Branch(branch) => branch.run(ctx).await,
                NodeContent::Step(step) => step.run(ctx).await,
                NodeContent::Act(act) => act.run(ctx).await,
            }?;

            ctx.emit_task(&ctx.task()).await?;
        }

        Ok(())
    }

    #[instrument(skip(self, ctx), fields(pid = %self.pid, tid = %self.id))]
    async fn next(&self, ctx: &Context) -> Result<NextAction> {
        debug!("task next");
        ctx.set_task(self);
        let task = ctx.task();

        // idempotent replay guard: skip if this task already propagated
        if self.is_propagation_done() {
            // close the re-dispatched outbox record: the applied phase is
            // already durable, so the re-run is a no-op
            if let Err(err) = self.runtime().complete_next(self).await {
                error!(error = %err, "complete_next failed");
            }
            return Ok(NextAction::Continue);
        }

        // 1. check uses action completed
        let mut next_action = task.check_uses_action(ctx).await?;

        // 2. check run into children
        if next_action.is_continue() {
            next_action = task.check_in_children(ctx).await?;
        }

        // 3. auto-complete task state
        if next_action.is_continue() {
            next_action = task.auto_complete(ctx).await?;
        }

        // 4. schedule next task
        if next_action.is_continue() && task.is_next() {
            next_action = match &self.node.content {
                NodeContent::Workflow(data) => data.next(ctx).await?,
                NodeContent::Step(data) => data.next(ctx).await?,
                NodeContent::Branch(data) => data.next(ctx).await?,
                NodeContent::Act(data) => data.next(ctx).await?,
            };
        }

        debug!(action = %next_action, "next action");

        if task.state().is_completed() {
            // terminal + emitted → propagation applied, mark idempotent and
            // close the durable outbox record (persisting the phase first)
            self.set_propagation_phase(PropagationPhase::Applied);
            if let Err(err) = self.runtime().complete_next(self).await {
                error!(error = %err, "complete_next failed");
            }
        }
        // non-terminal outcomes (children in flight, interrupt, …) deliberately
        // leave the record `Pending` so recovery re-dispatches this `next`.

        // 5. move to parent and continue
        if next_action.is_parent() {
            let parent = task.parent();
            if let Some(p) = &parent.clone() {
                let outputs = task.outputs();
                // the finished task's outputs fold into its parent — one hop;
                // the parent's own `next` carries them further up in turn
                p.update_data(&outputs);
                return Box::pin(p.next(ctx)).await;
            }
        }

        Ok(NextAction::Continue)
    }

    async fn on_error(&self, ctx: &Context) -> Result<()> {
        ctx.set_task(self);
        match &self.node.content {
            // boxed: an errored act's handler can bubble back into this task's
            // `on_error` through `Context::emit_error` (async recursion)
            NodeContent::Workflow(data) => Box::pin(data.on_error(ctx)).await,
            NodeContent::Step(data) => Box::pin(data.on_error(ctx)).await,
            NodeContent::Branch(data) => Box::pin(data.on_error(ctx)).await,
            NodeContent::Act(data) => Box::pin(data.on_error(ctx)).await,
        }
    }

    async fn on_timeout(&self, ctx: &Context) -> Result<()> {
        ctx.set_task(self);
        match &self.node.content {
            NodeContent::Workflow(data) => data.on_timeout(ctx).await,
            NodeContent::Step(data) => data.on_timeout(ctx).await,
            NodeContent::Branch(data) => data.on_timeout(ctx).await,
            NodeContent::Act(data) => data.on_timeout(ctx).await,
        }
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("name", &self.node.name())
            .field("type", &self.node.kind())
            .field("pid", &self.pid)
            .field("nid", &self.node.id())
            .field("state", &self.state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("prev", &self.prev_id())
            .field("next", &self.next_ids())
            .field("parent", &self.parent_id())
            .field("data", &self.data())
            .field("sealed", &self.sealed_data.read().clone())
            .field("err", &self.err())
            .finish()
    }
}

impl Task {
    pub fn data(&self) -> Vars {
        self.data.read().clone()
    }

    pub fn vars(&self) -> Vars {
        // Build a lineage chain once and merge each scope exactly once. The
        // previous recursive merge cloned intermediate maps at every level,
        // making deep task chains quadratic in the number of merged vars.
        //
        // The nearest scope is applied last, so it wins — the same precedence
        // [`Self::find`] reads with: an ancestor's value only fills a key this
        // scope does not hold. A write stays in the scope that made it (see
        // [`Self::update_data`]), so the two paths agree by merge order, not
        // by mirroring the value into the declaring ancestor.
        let mut chain = Vec::new();
        let mut cursor = self.parent();
        while let Some(task) = cursor {
            cursor = task.parent();
            chain.push(task.data());
        }

        // root first, then down to the nearest parent, then this scope
        let mut vars = Vars::new();
        while let Some(data) = chain.pop() {
            vars = vars.extend(data);
        }
        vars.extend(self.data())
    }

    pub fn with_data<T, F: Fn(&Vars) -> T>(&self, f: F) -> T {
        let data = self.data.read();
        f(&data)
    }

    /// Mark the scope's vars as diverged. The generation is bumped before the
    /// dirty flag so a persist that is writing the row concurrently can never
    /// clear this mutation away (see [`Self::clear_vars_dirty`]).
    fn mark_vars_dirty(&self) {
        self.vars_gen.fetch_add(1, Ordering::Release);
        self.vars_dirty.store(true, Ordering::Release);
    }

    pub fn set_data_with<F: Fn(&mut Vars)>(&self, f: F) {
        let mut data = self.data.write();
        f(&mut data);
        self.mark_vars_dirty();
    }

    pub fn set_data(&self, vars: &Vars) {
        let mut data = self.data.write();
        for (name, value) in vars.iter() {
            data.set(name, value);
        }
        self.mark_vars_dirty();
    }

    /// Restore-time write of a scope's persisted vars — fills the in-memory
    /// vars without marking them dirty (the row they came from is current).
    pub(crate) fn set_pure_data(&self, vars: &Vars) {
        let mut data = self.data.write();
        for (name, value) in vars.iter() {
            data.set(name, value);
        }
    }

    pub(crate) fn set_sealed(&self, name: &str, value: Vars) {
        let mut sealed = self.sealed_data.write();
        sealed.set(name, value);
        self.mark_vars_dirty();
    }

    /// Restore-time write of a scope's persisted sealed vars.
    pub(crate) fn set_pure_sealed_data(&self, vars: &Vars) {
        let mut data = self.sealed_data.write();
        for (name, value) in vars.iter() {
            data.set(name, value);
        }
    }

    /// The scope's vars row (data + sealed) diverged from the store since its
    /// last flush.
    pub fn is_vars_dirty(&self) -> bool {
        self.vars_dirty.load(Ordering::Acquire)
    }

    /// Generation counter bumped by every vars mutation since the task was
    /// built. The persist path reads it before serializing the row and clears
    /// the dirty flag only when it is unchanged afterwards — a mutation that
    /// raced the write keeps the scope dirty so its data is persisted next.
    pub(crate) fn vars_gen(&self) -> u64 {
        self.vars_gen.load(Ordering::Acquire)
    }

    /// Mark the scope's vars row as flushed. Called by the persist path right
    /// after the vars row was durably written.
    pub(crate) fn clear_vars_dirty(&self) {
        self.vars_dirty.store(false, Ordering::Release);
    }

    /// Get sealed data by resolver name. Walks the parent chain
    /// if not found locally (child overrides parent).
    pub fn sealed(&self, name: &str) -> Option<Vars> {
        // check local first
        if let Some(v) = self.sealed_data.read().get::<Vars>(name) {
            return Some(v);
        }
        // walk up parent chain
        let mut parent = self.parent();
        while let Some(task) = parent {
            if let Some(v) = task.sealed_data.read().get::<Vars>(name) {
                return Some(v);
            }
            parent = task.parent();
        }
        None
    }
    /// Whether this task's own row carries sealed data for `name` (does not
    /// walk the parent chain) — used to keep the first-sealed value on retry.
    pub(crate) fn has_sealed_local(&self, name: &str) -> bool {
        self.sealed_data.read().get_value(name).is_some()
    }

    pub fn has_sealed(&self) -> bool {
        !self.sealed_data.read().is_empty()
    }

    pub fn sealed_keys(&self) -> Vec<String> {
        self.sealed_data.read().keys().cloned().collect()
    }

    pub fn find<T>(&self, name: &str) -> Option<T>
    where
        T: DeserializeOwned + std::fmt::Debug + Clone,
    {
        let result = self.with_data(move |data| data.get(name));
        if result.is_some() {
            return result;
        }

        let mut parent = self.parent();
        while let Some(task) = parent {
            let result = task.with_data(|data| data.get::<T>(name));
            if result.is_some() {
                return result;
            }
            parent = task.parent();
        }
        None
    }

    /// Write `vars` into this scope's own data.
    ///
    /// A key an ancestor scope also holds is written **here only**: the value
    /// reaches the ancestors in turn, through the propagation of this task's
    /// outputs ([`Self::outputs`]) when its `next` hands the terminal outcome
    /// to the parent (`p.update_data(&outputs)`). Nothing walks the parent
    /// chain at write time, so a sibling subtree never observes this scope's
    /// writes and the parents' rows stay their own.
    ///
    /// Reads are unchanged ([`Self::vars`] and [`Self::find`] merge the parent
    /// chain), so this scope still sees everything an ancestor holds.
    pub fn update_data(&self, vars: &Vars) {
        self.set_data(vars);
    }

    fn move_next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        if let Some(next) = &task.node.next().upgrade() {
            ctx.schedule_once(next, ctx.task())?;
            return Ok(true);
        }

        Ok(false)
    }
}
