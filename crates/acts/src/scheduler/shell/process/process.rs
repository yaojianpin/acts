use crate::{
    Error, NodeKind, Result, ShareLock, Vars, Workflow,
    scheduler::{
        core::model::{legacy_task_state::LegacyTaskState, view::NodeTree},
        shell::{Context, Task, task_tree::TaskTree},
    },
    utils::{self, consts},
};

use parking_lot::RwLock;
use serde::Deserialize;
use std::{fmt, sync::Arc};

#[derive(Clone)]
pub struct Process {
    id: String,
    tree: ShareLock<NodeTree>,
    tasks: ShareLock<TaskTree>,
    state: ShareLock<LegacyTaskState>,
    start_time: ShareLock<i64>,
    err: ShareLock<Option<Error>>,
    end_time: ShareLock<i64>,
    timestamp: i64,
    env: ShareLock<Vars>,
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proc")
            .field("pid", &self.id)
            .field("mid", &self.with_model(|model| model.id.clone()))
            .field("state", &self.state())
            .field("err", &self.err())
            .field("env", &self.env())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

impl Process {
    pub fn new(pid: &str) -> Arc<Self> {
        Self::new_with_timestamp(pid, utils::time::timestamp())
    }

    pub fn new_with_timestamp(pid: &str, timestamp: i64) -> Arc<Self> {
        let tree = NodeTree::new();
        Arc::new(Process {
            id: pid.to_string(),
            tree: Arc::new(RwLock::new(tree)),
            state: Arc::new(RwLock::new(LegacyTaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            tasks: Arc::new(RwLock::new(TaskTree::new())),
            timestamp,
            env: Arc::new(RwLock::new(Vars::new())),
            err: Arc::new(RwLock::new(None)),
        })
    }

    pub fn data(&self) -> Vars {
        if let Some(root) = self.root() {
            return root.data();
        }
        Vars::new()
    }

    pub fn set_data(&self, vars: &Vars) {
        if let Some(root) = self.root() {
            root.set_data(vars);
        }
    }

    pub fn load(&self, model: &Workflow) -> Result<()> {
        let tree = &mut self.tree.write();
        tree.load(model)
    }

    pub(crate) fn load_owned(&self, model: Workflow) -> Result<()> {
        let tree = &mut self.tree.write();
        tree.load_owned(model)
    }

    pub(crate) fn load_with_vars(&self, model: &Workflow, vars: &Vars) -> Result<()> {
        let mut model = model.clone();
        model.set_vars(vars);
        let tree = &mut self.tree.write();
        tree.load_owned(model)
    }

    pub fn tree(&self) -> parking_lot::RwLockReadGuard<'_, NodeTree> {
        self.tree.read()
    }

    pub(crate) fn with_model<T>(&self, f: impl FnOnce(&Workflow) -> T) -> T {
        f(&self.tree().model)
    }

    pub fn state(&self) -> LegacyTaskState {
        self.state.read().clone()
    }

    pub fn err(&self) -> Option<Error> {
        self.err.read().clone()
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read()
    }
    pub fn env(&self) -> Vars {
        let env = self.env.read();
        env.clone()
    }

    pub fn with_env<T, F: FnOnce(&Vars) -> T>(&self, f: F) -> T
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        let env = self.env.read();
        f(&env)
    }

    pub fn with_env_mut<F: FnOnce(&mut Vars)>(&self, f: F) {
        let mut env = self.env.write();
        f(&mut env)
    }

    /// Seal a caller's snapshot scope authority into the process env.
    ///
    /// Written once, under a private (`__`) key: the workflow's `$env` proxy
    /// refuses private keys, so a model can neither read the credential nor
    /// overwrite it. The env row is persisted with the proc, so the authority
    /// survives a restart and a resumed process keeps the scope it started
    /// with. A process started without a credential (a `schedule` trigger, an
    /// embedder calling `Runtime::start` itself) carries none — see
    /// [`Process::owner_scope`].
    pub(crate) fn set_owner_scope(&self, policy: &crate::ScopePolicy) {
        self.with_env_mut(|env| env.set(consts::PROC_OWNER, policy.clone()));
    }

    /// The owner's snapshot scope authority. A process without one reads
    /// nothing ([`crate::ScopePolicy::default`] is `deny_all`): it was started
    /// with no caller authority to seal — a `schedule` trigger, or an
    /// embedder that called `Runtime::start` instead of going through
    /// [`crate::Engine::executor`] — or its row predates this field.
    pub(crate) fn owner_scope(&self) -> crate::ScopePolicy {
        self.with_env(|env| env.get::<crate::ScopePolicy>(consts::PROC_OWNER))
            .unwrap_or_default()
    }

    /// Seal the directory this process's filesystem access is confined to —
    /// `<workdir>/<pid>`, the process's own directory, not the root the
    /// engine config declares — under a private env key so the workflow's `$env`
    /// proxy can neither read nor overwrite it. Persisted with the process
    /// env, so a resumed process keeps the same directory.
    pub(crate) fn set_workdir(&self, dir: &std::path::Path) {
        self.with_env_mut(|env| env.set(consts::PROC_WORKDIR, dir));
    }

    /// The process's own directory (`<workdir>/<pid>`), or `None` when the
    /// engine config declares no workdir root (and for process rows
    /// written before this field existed). `Context::workdir` and
    /// `$env.WORK_DIR` both answer this.
    pub fn workdir(&self) -> Option<std::path::PathBuf> {
        self.with_env(|env| env.get::<std::path::PathBuf>(consts::PROC_WORKDIR))
    }

    pub fn inputs(self: &Arc<Self>) -> Vars {
        if let Some(task) = self.root() {
            let ctx = task.create_context(self);
            let task = ctx.task();
            return self.with_model(|model| utils::fill_proc_vars(&task, &model.vars(), &ctx));
        }
        Vars::new()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn root(&self) -> Option<Arc<Task>> {
        // The root task's id is its instance's wire since C5, so the task is
        // found on the model's workflow node, not by a literal `$` id.
        self.find_tasks(|task| task.node().kind() == NodeKind::Workflow)
            .into_iter()
            .next()
    }

    pub fn task(&self, tid: &str) -> Option<Arc<Task>> {
        self.tasks.read().task_by_tid(tid)
    }

    pub fn find_tasks(&self, predicate: impl Fn(&Arc<Task>) -> bool) -> Vec<Arc<Task>> {
        let tasks = self.tasks.read();
        let mut ret = tasks.find_tasks(predicate);
        ret.sort_by_key(|a| a.start_time());

        ret
    }

    pub fn tasks(&self) -> Vec<Arc<Task>> {
        let ttree = self.tasks.read();
        ttree.tasks()
    }

    pub fn task_by_nid(&self, nid: &str) -> Vec<Arc<Task>> {
        self.find_tasks(|t| t.node().id() == nid || t.node().content.id() == nid)
    }

    pub fn create_context(self: &Arc<Self>, task: &Arc<Task>) -> Context {
        Context::new(self, task)
    }

    pub(crate) fn set_start_time(&self, time: i64) {
        *self.start_time.write() = time;
    }

    pub(crate) fn set_pure_state(&self, state: LegacyTaskState) {
        *self.state.write() = state;
    }

    pub(crate) fn set_pure_err(&self, err: &Error) {
        *self.err.write() = Some(err.clone());
    }

    pub(crate) fn set_env(&self, value: &Vars) {
        *self.env.write() = value.clone();
    }

    pub fn push_task(&self, task: Arc<Task>) -> Result<()> {
        let mut tasks = self.tasks.write();
        tasks.push(task)?;
        Ok(())
    }
}
