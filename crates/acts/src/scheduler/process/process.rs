use crate::ActTask;
use crate::event::EventAction;
use crate::store::DbCollectionIden;
use crate::{
    ActError, Error, NodeKind, ProcInfo, Result, ShareLock, Vars, Workflow, data,
    event::Action,
    scheduler::{
        Context, Runtime, Task, TaskState,
        tree::{Node, NodeTree, TaskTree},
    },
    utils::{self, consts},
};
use parking_lot::RwLock;
use serde::Deserialize;
use std::{cell::RefCell, fmt, sync::Arc};
use tokio::time::Duration;
use tracing::{debug, error, info, instrument};

#[derive(Clone)]
pub struct Process {
    id: String,
    tree: ShareLock<NodeTree>,
    tasks: ShareLock<TaskTree>,
    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    err: ShareLock<Option<Error>>,
    end_time: ShareLock<i64>,
    timestamp: i64,
    env: ShareLock<Vars>,
    runtime: Arc<Runtime>,
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proc")
            .field("pid", &self.id)
            .field("mid", &self.model().id)
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
    pub fn new(pid: &str, rt: &Arc<Runtime>) -> Arc<Self> {
        Self::new_with_timestamp(pid, utils::time::timestamp(), rt)
    }

    pub fn new_with_timestamp(pid: &str, timestamp: i64, rt: &Arc<Runtime>) -> Arc<Self> {
        let tree = NodeTree::new();
        Arc::new(Process {
            id: pid.to_string(),
            tree: Arc::new(RwLock::new(tree)),
            state: Arc::new(RwLock::new(TaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            tasks: Arc::new(RwLock::new(TaskTree::new())),
            timestamp,
            env: Arc::new(RwLock::new(Vars::new())),
            err: Arc::new(RwLock::new(None)),
            runtime: rt.clone(),
        })
    }

    pub fn data(&self) -> Vars {
        if let Some(root) = self.root() {
            return root.data();
        }
        Vars::new()
    }

    pub fn set_data_with<F: Fn(&mut Vars)>(&self, f: F) {
        if let Some(root) = self.root() {
            root.set_data_with(f);
        }
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

    pub fn tree(&self) -> parking_lot::RwLockReadGuard<'_, NodeTree> {
        self.tree.read()
    }

    pub fn model(&self) -> Box<Workflow> {
        self.tree().model.clone()
    }

    pub fn state(&self) -> TaskState {
        self.state.read().clone()
    }

    pub fn set_err(&self, err: &Error) {
        *self.err.write() = Some(err.clone());
        self.set_state(TaskState::Error);
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
    pub fn timestamp(&self) -> i64 {
        self.timestamp
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

    pub fn outputs(&self) -> Vars {
        if let Some(root) = self.root() {
            return root.outputs();
        }

        Vars::new()
    }

    pub fn inputs(&self) -> Vars {
        if let Some(task) = self.root() {
            let ctx = task.create_context();
            let vars = utils::fill_proc_vars(&task, &self.model().vars(), &ctx);
            return vars;
        }
        Vars::new()
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }
        utils::time::time_millis() - self.start_time()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn info(&self) -> ProcInfo {
        let workflow = self.model();
        ProcInfo {
            id: self.id.clone(),
            name: workflow.name.clone(),
            mid: workflow.id.clone(),
            state: self.state().into(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            timestamp: self.timestamp,
            tasks: Vec::new(),
        }
    }

    pub fn root(&self) -> Option<Arc<Task>> {
        self.task(consts::TASK_ROOT_TID)
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

    pub fn node(&self, nid: &str) -> Option<Arc<Node>> {
        self.tree().node(nid)
    }

    pub fn tasks(&self) -> Vec<Arc<Task>> {
        let ttree = self.tasks.read();
        ttree.tasks()
    }

    pub fn children(&self, tid: &str) -> Vec<Arc<Task>> {
        let mut tasks = self
            .tasks()
            .into_iter()
            .filter(|iter| iter.parent_id() == Some(tid.to_string()))
            .collect::<Vec<_>>();

        tasks.sort_by_key(|a| a.timestamp);
        tasks
    }

    pub fn task_by_nid(&self, nid: &str) -> Vec<Arc<Task>> {
        self.find_tasks(|t| t.node().id() == nid || t.node().content.id() == nid)
    }

    /// Find the task instance created for `node` with `prev` as its previous
    /// task — the identity used when scheduling. A crash mid-`next` can leave
    /// such an instance behind; re-scheduling must reuse it instead of creating
    /// a duplicate (see `Context::schedule_once`).
    pub fn task_for_node_prev(&self, node_id: &str, prev_id: &str) -> Option<Arc<Task>> {
        self.find_tasks(|t| t.node().id() == node_id && t.prev_id().as_deref() == Some(prev_id))
            .into_iter()
            .next()
    }

    #[cfg(test)]
    pub fn task_by_params(&self, key: &str, value: &str) -> Vec<Arc<Task>> {
        self.find_tasks(|t| {
            use serde_json::Value;

            let params = t.node().params();
            if t.is_kind(NodeKind::Act)
                && params != Value::Null
                && let Some(v) = params.get::<&str>(key)
                && let Some(s) = v.as_str()
            {
                return value == s;
            }

            false
        })
    }

    #[cfg(test)]
    pub fn task_by_uses(&self, uses: &str) -> Vec<Arc<Task>> {
        self.find_tasks(|t| t.node().uses().as_deref() == Some(uses))
    }

    pub fn create_context(self: &Arc<Self>, task: &Arc<Task>) -> Context {
        Context::new(self, task)
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            self.set_end_time(utils::time::time_millis());
        } else if state.is_running() {
            self.set_start_time(utils::time::time_millis());
        }
        *self.state.write() = state;
    }

    pub(crate) fn set_start_time(&self, time: i64) {
        *self.start_time.write() = time;
    }
    pub(crate) fn set_end_time(&self, time: i64) {
        *self.end_time.write() = time;
    }

    pub(crate) fn set_pure_state(&self, state: TaskState) {
        *self.state.write() = state;
    }

    pub(crate) fn set_pure_err(&self, err: &Error) {
        *self.err.write() = Some(err.clone());
    }

    pub(crate) fn set_env(&self, value: &Vars) {
        *self.env.write() = value.clone();
    }

    pub(crate) async fn do_tick(&self) {
        // only run the timeout check for tasks that are running or interrupted, since
        // tasks that are completed or skipped will not be timed out
        let tasks = self.find_tasks(|t| {
            t.is_timeouts() && (t.state().is_running() || t.state().is_interrupted())
        });
        for t in tasks.iter() {
            let ctx = t.create_context();
            if let Err(err) = t.on_timeout(&ctx).await {
                error!(error = %err, "tick failed");
            }
        }
    }
    #[instrument(skip(self, action), fields(pid = %self.id, tid = %action.tid, event = ?action.event))]
    pub async fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        debug!("action received");
        let mut action = action.clone();
        let task = self.task(&action.tid).ok_or(ActError::Action(format!(
            "cannot find task by '{}' tasks={:?}",
            action.tid,
            self.tasks()
        )))?;

        if action.event == EventAction::Push {
            if !task.is_kind(NodeKind::Step) {
                return Err(ActError::Action(format!(
                    "The task '{}' is not an Step task",
                    action.tid
                )));
            }
        } else if !task.is_kind(NodeKind::Act) {
            return Err(ActError::Action(format!(
                "The task '{}' is not an Act task",
                action.tid
            )));
        }

        // filter the data by exposes
        let exposes = task.node().content.exposes();
        if !exposes.is_empty() {
            let mut options = Vars::new();
            for var in exposes {
                if !action.options.contains_key(&var.name) {
                    return Err(ActError::Action(format!(
                        "the options is not satisfied with act's outputs '{}' in task({})",
                        var.name, action.tid
                    )));
                }
                if let Some(value) = action.options.get_value(&var.name) {
                    options.set(&var.name, value.clone());
                }
            }

            // reset the options by rets definition
            action.options = options;
        }

        let ctx = task.create_context();
        ctx.set_action(&action)?;
        task.update(&ctx).await?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        // One-shot atomic start. The state lock guards the `None -> Running`
        // transition, so concurrent or repeated `start()` calls can never run
        // the start body twice: the root task is scheduled exactly once and
        // exactly one per-process tick loop is spawned. A call that finds the
        // process already started (or finished) is a no-op — it must not
        // replace the root task, reset the start time, or re-push the process.
        {
            let mut state = self.state.write();
            if !state.is_none() {
                return Ok(());
            }
            *state = TaskState::Running;
            *self.start_time.write() = utils::time::time_millis();
        }

        info!(pid = %self.id, mid = %self.tree().model.id, name = %self.tree().model.name, "process started");
        let cache = self.runtime.cache().clone();
        let proc = self.clone();

        // Build the root task first, then persist the proc row and the root
        // task row as ONE atomic store batch (a crash can never leave a
        // durable proc row without its root task row — which would resume as
        // an un-runnable, task-less process), cache the process and finally
        // dispatch the root task to the in-memory queue.
        let root = {
            let tr = self.tree();
            match &tr.root {
                Some(root) => Some(self.create_task(root, None)?),
                None => None,
            }
        };

        cache.start_proc(&proc, root.as_ref()).await?;

        // Start per-process tick loop
        self.init_tick();

        if let Some(task) = root {
            self.runtime.dispatch_root(&task)?;
        }

        Ok(())
    }

    /// Create a task for the node, rejecting the scheduling when the node has
    /// already been executed `max_node_run_times` times in this process (0
    /// disables the check). A node whose `next` points back at itself — or
    /// into a cycle — would otherwise create an unbounded stream of new tasks;
    /// the rejected scheduling errors the process instead.
    pub fn create_task(
        self: &Arc<Process>,
        node: &Arc<Node>,
        prev: Option<Arc<Task>>,
    ) -> Result<Arc<Task>> {
        let mut tid = utils::shortid();
        if node.kind() == NodeKind::Workflow {
            // set $ for the root task id
            // a process only has one root task
            tid = consts::TASK_ROOT_TID.to_string();
        }
        let task = Arc::new(Task::new(self, &tid, node.clone(), &self.runtime));
        if let Some(prev) = prev {
            task.set_prev(&prev.id);
            prev.set_next(&tid);

            if task.node().level > prev.node().level {
                task.set_parent(&prev.id);
            } else if let Some(parent) = prev.parent() {
                task.set_parent(&parent.id);
            }
        }

        // the run-limit check and the registration must be atomic so
        // concurrent schedulers cannot overshoot the limit
        let mut tasks = self.tasks.write();
        let max = self.runtime.config().max_node_run_times();
        if max > 0 && tasks.run_count(node.id()) >= max as usize {
            return Err(ActError::Runtime(format!(
                "node '{}' ({}) in process '{}' was executed more than {} times, \
                 a loop in the workflow is likely",
                node.id(),
                node.name(),
                self.id,
                max
            )));
        }
        tasks.push(task.clone())?;
        Ok(task)
    }

    pub fn push_task(&self, task: Arc<Task>) -> Result<()> {
        let mut tasks = self.tasks.write();
        tasks.push(task)?;
        Ok(())
    }

    pub fn parent(&self) -> Option<(String, String)> {
        if let Some(root) = &self.root() {
            let use_data = root.with_data(|data| {
                (
                    data.get::<String>(consts::ACT_USE_PARENT_PROC_ID),
                    data.get::<String>(consts::ACT_USE_PARENT_TASK_ID),
                )
            });

            if let (Some(ppid), Some(ptid)) = use_data {
                return Some((ppid, ptid));
            }
        }

        None
    }

    #[allow(unused)]
    pub fn print(&self) {
        println!("Proc({})  state={}", self.id, self.state());
        println!("data={}", self.data());
        println!("{}", self.tree_output());
    }

    #[allow(unused)]
    pub fn tree_output(&self) -> String {
        let s = &RefCell::new(String::new());
        s.borrow_mut()
            .push_str(&format!("Proc({})  state={}\n", self.id, self.state()));

        if let Some(root) = self.root() {
            self.write_task_tree(&root, s, 0, true);
        }

        s.clone().into_inner()
    }

    fn write_task_tree(&self, task: &Arc<Task>, s: &RefCell<String>, depth: usize, is_last: bool) {
        // indent with tree connectors
        s.borrow_mut().push_str(&"    ".repeat(depth));
        if depth > 0 {
            if is_last {
                s.borrow_mut().push_str("└── ");
            } else {
                s.borrow_mut().push_str("├── ");
            }
        }

        s.borrow_mut().push_str(&format!(
            "Task({}) prev={} kind={} nid={} name={} state={}  uses={}  data={}\n",
            task.id,
            match task.prev_id() {
                Some(v) => v,
                None => "nil".to_string(),
            },
            task.node().kind(),
            task.node().id(),
            task.node().content.name(),
            task.state(),
            task.node().uses().as_deref().unwrap_or("nil"),
            task.data()
        ));

        let children = self.children(&task.id);
        let len = children.len();
        for (i, child) in children.iter().enumerate() {
            self.write_task_tree(child, s, depth + 1, i == len - 1);
        }
    }

    pub fn into_data(self: &Arc<Self>) -> Result<data::Proc> {
        let model = self.model();
        Ok(data::Proc {
            id: self.id.clone(),
            model: model.to_json()?,
            mid: model.id,
            name: model.name,
            state: self.state().into(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            timestamp: self.timestamp(),
            env: self.env().to_string(),
            err: self.err().map(|err| err.to_string()),
            v: data::Proc::version(),
        })
    }

    fn init_tick(&self) {
        // Start per-process tick loop
        #[cfg(not(test))]
        let interval_ms = {
            let config = self.runtime.config();
            let secs = if config.tick_interval_secs() > 0 {
                config.tick_interval_secs()
            } else {
                15
            };
            (secs * 1000) as u64
        };
        #[cfg(test)]
        let interval_ms = 800u64;

        let tick_proc = self.clone();
        let shutdown = self.runtime.shutdown_token();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => {}
                }
                if !tick_proc.state().is_running() {
                    break;
                }
                tick_proc.do_tick().await;
            }
        });
    }
}
