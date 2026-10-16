use crate::{
    Error, NodeKind, Result, ShareLock, Variant, Vars, data,
    scheduler::{
        Runtime,
        core::model::{legacy_task_state::LegacyTaskState, view::Node},
        shell::{Context, Process},
    },
    store::DbCollectionIden,
    utils::{self, consts},
};

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

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
    state: ShareLock<LegacyTaskState>,

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

    node: Arc<Node>,

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
            state: Arc::new(RwLock::new(LegacyTaskState::None)),
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
            runtime: rt.clone(),
        }
    }

    pub fn unique_id(&self) -> String {
        format!("{}:{}", self.pid, self.id)
    }

    pub(crate) fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
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

    pub fn state(&self) -> LegacyTaskState {
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

    pub fn create_context(self: &Arc<Self>, proc: &Arc<Process>) -> Context {
        proc.create_context(self)
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

    pub fn parent(&self, proc: &Arc<Process>) -> Option<Arc<Task>> {
        let parent = self.parent.read().clone()?;
        proc.task(&parent)
    }

    pub fn children(&self, proc: &Arc<Process>) -> Vec<Arc<Self>> {
        proc.children(&self.id)
    }

    pub fn next(&self, proc: &Arc<Process>) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        let nexts = self.next_ids();
        for tid in &nexts {
            if let Some(task) = proc.task(tid) {
                ret.push(task);
            }
        }
        ret
    }

    pub fn siblings(&self, proc: &Arc<Process>) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        if let Some(parent) = self.parent(proc) {
            let children = parent.children(proc);
            ret.extend(children.iter().filter(|iter| iter.id != self.id).cloned());
        }

        ret
    }

    pub fn inputs(self: &Arc<Self>, ctx: &Context) -> Vars {
        let mut inputs = Vars::new();
        if let Some(prev) = self.prev_id()
            && let Some(prev_task) = ctx.proc.task(&prev)
        {
            // set the prev task's outputs as current inputs. The outputs are
            // read in the prev task's own scope (its context, not `ctx`): the
            // exposes/expressions it fills are its own.
            let prev_ctx = ctx.proc.create_context(&prev_task);
            for (k, v) in prev_task.outputs(&prev_ctx).iter() {
                inputs.set(k, v.clone());
            }
        }
        // merge the node vars
        let vars = utils::fill_inputs(&self.node.content.vars(), ctx);
        inputs.extend(vars)
    }

    pub fn outputs(self: &Arc<Self>, ctx: &Context) -> Vars {
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
            for (key, _) in self.data().iter() {
                if !consts::is_private_key(key) {
                    outputs.set(key, json!(null))
                }
            }
        }

        utils::fill_outputs(&outputs, ctx)
    }

    pub fn options(self: &Arc<Self>) -> Vars {
        self.node.content.options()
    }

    pub fn params(self: &Arc<Self>, ctx: &Context) -> serde_json::Value {
        utils::fill_params(&self.node.content.params(), ctx)
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

    /// Restore an error read off a task row without touching the state: the
    /// row's own `state` column is restored separately, so a load must not
    /// let the error imply `Error`.
    pub(crate) fn set_pure_err(&self, err: &Error) {
        *self.err.write() = Some(err.clone());
    }

    pub fn err(&self) -> Option<Error> {
        self.err.read().clone()
    }

    pub fn set_pure_state(&self, state: LegacyTaskState) {
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

    pub fn vars(&self, proc: &Arc<Process>) -> Vars {
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
        let mut cursor = self.parent(proc);
        while let Some(task) = cursor {
            cursor = task.parent(proc);
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

    /// Get sealed data by resolver name. Walks the parent chain
    /// if not found locally (child overrides parent).
    pub fn sealed(&self, proc: &Arc<Process>, name: &str) -> Option<Vars> {
        // check local first
        if let Some(v) = self.sealed_data.read().get::<Vars>(name) {
            return Some(v);
        }
        // walk up parent chain
        let mut parent = self.parent(proc);
        while let Some(task) = parent {
            if let Some(v) = task.sealed_data.read().get::<Vars>(name) {
                return Some(v);
            }
            parent = task.parent(proc);
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

    pub fn find<T>(&self, proc: &Arc<Process>, name: &str) -> Option<T>
    where
        T: DeserializeOwned + std::fmt::Debug + Clone,
    {
        let result = self.with_data(move |data| data.get(name));
        if result.is_some() {
            return result;
        }

        let mut parent = self.parent(proc);
        while let Some(task) = parent {
            let result = task.with_data(|data| data.get::<T>(name));
            if result.is_some() {
                return result;
            }
            parent = task.parent(proc);
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
}
