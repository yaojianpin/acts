use crate::{
    data,
    event::Action,
    sch::{
        tree::{Node, NodeTree, TaskTree},
        Context, Runtime, Task, TaskLifeCycle, TaskState,
    },
    utils::{self, consts},
    ActError, Error, NodeKind, ProcInfo, Result, ShareLock, Vars, Workflow,
};
use serde::Deserialize;
use std::{
    cell::RefCell,
    fmt,
    sync::{Arc, RwLock},
};
use tracing::{debug, error, instrument};

#[derive(Clone)]
pub struct Proc {
    id: String,
    tree: ShareLock<NodeTree>,
    tasks: ShareLock<TaskTree>,
    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    err: ShareLock<Option<Error>>,
    end_time: ShareLock<i64>,
    timestamp: i64,
    env_local: ShareLock<Vars>,
    runtime: Arc<Runtime>,
    // cache: Arc<Cache>,
    // sync: Arc<std::sync::Mutex<usize>>,
}

impl std::fmt::Debug for Proc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proc")
            .field("pid", &self.id)
            .field("mid", &self.model().id)
            .field("state", &self.state())
            .field("err", &self.err())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

impl Proc {
    pub fn new(pid: &str, rt: &Arc<Runtime>) -> Arc<Self> {
        Self::new_with_timestamp(pid, utils::time::timestamp(), rt)
    }

    pub fn new_with_timestamp(pid: &str, timestamp: i64, rt: &Arc<Runtime>) -> Arc<Self> {
        let tree = NodeTree::new();
        // let cache = r.scher().cache();
        let proc = Arc::new(Proc {
            id: pid.to_string(),
            tree: Arc::new(RwLock::new(tree)),
            state: Arc::new(RwLock::new(TaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            tasks: Arc::new(RwLock::new(TaskTree::new())),
            // sync: Arc::new(std::sync::Mutex::new(0)),
            timestamp: timestamp,
            env_local: Arc::new(RwLock::new(Vars::new())),
            err: Arc::new(RwLock::new(None)),
            runtime: rt.clone(),
            // cache: cache.clone(),
        });

        // push the proc to cache
        // cache.push_proc(&proc);

        proc
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
        let tree = &mut self.tree.write().unwrap();
        tree.load(model)
    }

    pub fn tree(&self) -> std::sync::RwLockReadGuard<'_, NodeTree> {
        self.tree.read().unwrap()
    }

    pub fn model(&self) -> Box<Workflow> {
        self.tree().model.clone()
    }

    pub fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    pub fn set_err(&self, err: &Error) {
        *self.err.write().unwrap() = Some(err.clone());
        self.set_state(TaskState::Error);
    }

    pub fn err(&self) -> Option<Error> {
        self.err.read().unwrap().clone()
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read().unwrap()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read().unwrap()
    }
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    pub fn env_local(&self) -> Vars {
        let env_local = self.env_local.read().unwrap();
        env_local.clone()
    }

    pub fn with_env_local<T, F: FnOnce(&Vars) -> T>(&self, f: F) -> T
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        let local = self.env_local.read().unwrap();
        f(&local)
    }

    pub fn with_env_local_mut<F: FnOnce(&mut Vars)>(&self, f: F) {
        let mut local = self.env_local.write().unwrap();
        f(&mut local)
    }

    pub fn outputs(&self) -> Vars {
        if let Some(root) = self.root() {
            return root.outputs();
        }

        Vars::new()
    }

    pub fn inputs(&self) -> Vars {
        let inputs = &self.model().inputs;
        if let Some(task) = self.root() {
            let ctx = task.create_context();
            let vars = utils::fill_proc_vars(&task, inputs, &ctx);
            return vars;
        }
        Vars::new()
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
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
            tasks: "".to_string(),
        }
    }

    pub fn root(&self) -> Option<Arc<Task>> {
        self.task(consts::TASK_ROOT_TID)
    }

    pub fn task(&self, tid: &str) -> Option<Arc<Task>> {
        self.tasks.read().unwrap().task_by_tid(tid)
    }

    pub fn find_tasks(&self, predicate: impl Fn(&Arc<Task>) -> bool) -> Vec<Arc<Task>> {
        let tasks = self.tasks.read().unwrap();
        let mut ret = tasks.find_tasks(predicate);
        ret.sort_by(|a, b| a.start_time().cmp(&b.start_time()));

        ret
    }

    pub fn node(&self, nid: &str) -> Option<Arc<Node>> {
        self.tree().node(nid)
    }

    pub fn tasks(&self) -> Vec<Arc<Task>> {
        let ttree = self.tasks.read().unwrap();
        ttree.tasks()
    }

    pub fn children(&self, tid: &str) -> Vec<Arc<Task>> {
        let mut tasks = self
            .tasks()
            .into_iter()
            .filter(|iter| iter.prev() == Some(tid.to_string()))
            .collect::<Vec<_>>();

        tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        tasks
    }

    pub fn task_by_nid(&self, nid: &str) -> Vec<Arc<Task>> {
        self.find_tasks(|t| t.node().id() == nid)
    }

    pub fn create_context(self: &Arc<Self>, task: &Arc<Task>) -> Context {
        Context::new(&self, task)
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            self.set_end_time(utils::time::time_millis());
        } else if state.is_running() {
            self.set_start_time(utils::time::time_millis());
        }
        *self.state.write().unwrap() = state;
    }

    pub(crate) fn set_start_time(&self, time: i64) {
        *self.start_time.write().unwrap() = time;
    }
    pub(crate) fn set_end_time(&self, time: i64) {
        *self.end_time.write().unwrap() = time;
    }

    pub(crate) fn set_pure_state(&self, state: TaskState) {
        *self.state.write().unwrap() = state;
    }

    pub(crate) fn set_pure_err(&self, err: &Error) {
        *self.err.write().unwrap() = Some(err.clone());
    }

    pub(crate) fn set_env_local(&self, value: &Vars) {
        *self.env_local.write().unwrap() = value.clone();
    }

    pub(crate) fn do_tick(&self) {
        self.find_tasks(|t| t.hooks().contains_key(&TaskLifeCycle::Timeout))
            .iter()
            .for_each(|t| {
                let ctx = t.create_context();
                t.run_hooks_timeout(&ctx).unwrap_or_else(|err| {
                    eprintln!("{}", err);
                    error!("{}", err);
                });
            });
    }

    #[instrument()]
    pub fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        let mut action = action.clone();
        let task = self.task(&action.tid).ok_or(ActError::Action(format!(
            "cannot find task by '{}' tasks={:?}",
            action.tid,
            self.tasks()
        )))?;

        if action.event == consts::EVT_PUSH {
            if !task.is_kind(NodeKind::Step) {
                return Err(ActError::Action(format!(
                    "The task '{}' is not an Step task",
                    action.tid
                )));
            }
        } else {
            if !task.is_kind(NodeKind::Act) {
                return Err(ActError::Action(format!(
                    "The task '{}' is not an Act task",
                    action.tid
                )));
            }
        }

        // check act return
        let rets = task.node().content.rets();
        if rets.len() > 0 {
            let mut options = Vars::new();
            for (key, _) in &rets {
                if !action.options.contains_key(key) {
                    return Err(ActError::Action(format!(
                        "the options is not satisfied with act's rets '{}' in task({})",
                        key, action.tid
                    )));
                }
                let value = action.options.get_value(key).unwrap();
                options.set(key, value.clone());
            }

            // retset the options by rets defination
            action.options = options;
        }

        let ctx = task.create_context();
        ctx.set_action(&action)?;
        task.update(&ctx)?;
        Ok(())
    }

    #[instrument()]
    pub fn do_task(self: &Arc<Self>, tid: &str, ctx: &Context) {
        debug!("do_task tid={}", tid);
        //let task = ctx.task();
        // task.exec(ctx).unwrap_or_else(|err| {
        //     eprintln!("error: {err}");
        //     task.set_err(&err.into());
        //     let _ = ctx.emit_error();
        // });
        if let Some(task) = &self.task(tid) {
            task.exec(ctx).unwrap_or_else(|err| {
                eprintln!("error: {err}");
                task.set_err(&err.into());
                let _ = ctx.emit_error();
            });
        } else {
            println!("cannot find task pid={} tid={}", self.id(), tid);
            println!("tasks={:?}", self.tasks());
        }
    }

    #[instrument()]
    pub fn start(self: &Arc<Self>) {
        // let _lock = self.sync.lock().unwrap();
        self.set_state(TaskState::Running);
        self.runtime.cache().push_proc(self);
        let tr = self.tree();
        if let Some(root) = &tr.root {
            let task = self.create_task(root, None);
            self.runtime.push(&task);
        }
    }

    pub fn create_task(self: &Arc<Proc>, node: &Arc<Node>, prev: Option<Arc<Task>>) -> Arc<Task> {
        let mut tid = utils::shortid();
        if node.kind() == NodeKind::Workflow {
            // set $ for the root task id
            // a proc only has one root task
            tid = consts::TASK_ROOT_TID.to_string();
        }
        let task = Arc::new(Task::new(&self, &tid, node.clone(), &self.runtime));
        if let Some(prev) = prev {
            task.set_prev(Some(prev.id.clone()));
        }
        self.push_task(task.clone());
        task
    }

    pub fn push_task(&self, task: Arc<Task>) {
        let mut tasks = self.tasks.write().unwrap();
        tasks.push(task);
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
        let ttree = self.tasks.read().unwrap();

        println!("Proc({})  state={}", self.id, self.state());
        println!("data={}", self.data());
        if let Some(root) = ttree.root() {
            self.visit(&root, |task| {
                let mut level = task.node().level;
                while level > 0 {
                    print!("  ");
                    level -= 1;
                }

                println!(
                    "Task({}) {}  nid={} name={} tag={} prev={} state={}  data={} err={:?}",
                    task.id,
                    task.node().typ(),
                    task.node().id(),
                    task.node().name(),
                    task.node().tag(),
                    match task.prev() {
                        Some(v) => v,
                        None => "nil".to_string(),
                    },
                    task.state(),
                    task.data(),
                    task.err(),
                );
            })
        }
    }

    #[allow(unused)]
    pub fn tree_output(&self) -> String {
        let ttree = self.tasks.read().unwrap();
        let s = &RefCell::new(String::new());
        s.borrow_mut()
            .push_str(&format!("Proc({})  state={}\n", self.id, self.state()));
        if let Some(root) = ttree.root() {
            self.visit(&root, move |task| {
                let mut level = task.node().level;
                while level > 0 {
                    s.borrow_mut().push_str("  ");
                    level -= 1;
                }
                s.borrow_mut().push_str(&format!(
                    "Task({}) prev={} kind={} nid={} name={} state={}\n",
                    task.id,
                    match task.prev() {
                        Some(v) => v,
                        None => "nil".to_string(),
                    },
                    task.node().kind(),
                    task.node().id(),
                    task.node().content.name(),
                    task.state(),
                ));
            })
        }

        s.clone().into_inner()
    }
    #[allow(unused)]
    pub fn visit<F: Fn(&Arc<Task>) + Clone>(&self, task: &Arc<Task>, f: F) {
        f(task);

        let tasks = task.children();
        for child in tasks {
            self.visit(&child, f.clone());
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
            env_local: self.env_local().to_string(),
            err: self.err().map(|err| err.to_string()),
        })
    }
}
