use crate::{
    env::Enviroment,
    event::{Action, EventAction},
    sch::{
        proc::Act,
        tree::TaskTree,
        tree::{Node, NodeTree},
        Context, Scheduler, Task, TaskState,
    },
    utils, ActError, ActResult, ActionState, ProcInfo, ShareLock, Vars, Workflow, WorkflowState,
};
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, RwLock},
};
use tracing::instrument;

#[derive(Clone)]
pub struct Proc {
    pub(in crate::sch) env: Arc<Enviroment>,
    pub(in crate::sch) tree: Arc<NodeTree>,

    pid: String,
    model: Arc<Workflow>,

    tasks: ShareLock<TaskTree>,
    acts: ShareLock<HashMap<String, Arc<Act>>>,

    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    sync: Arc<Mutex<i32>>,
}

impl std::fmt::Debug for Proc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proc")
            .field("pid", &self.pid)
            .field("mid", &self.model.id)
            .field("state", &self.state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .finish()
    }
}

impl Proc {
    pub fn new(pid: &str, workflow: &Workflow, state: &TaskState) -> Self {
        Proc::new_raw(workflow, pid, state)
    }

    pub fn new_raw(workflow: &Workflow, pid: &str, state: &TaskState) -> Self {
        let env = Arc::new(Enviroment::new());

        // set both env and outputs as global vars
        let vars = utils::fill_proc_vars(&env, &workflow.env);
        env.append(&workflow.outputs);
        env.append(&vars);

        let mut workflow = workflow.clone();
        let tr = NodeTree::build(&mut workflow);
        Proc {
            pid: pid.to_string(),
            env: env.clone(),
            model: Arc::new(workflow.clone()),
            tree: tr,
            state: Arc::new(RwLock::new(state.clone())),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            tasks: Arc::new(RwLock::new(TaskTree::new())),
            acts: Arc::new(RwLock::new(HashMap::new())),
            sync: Arc::new(Mutex::new(0)),
        }
    }

    pub fn append_vars(&self, vars: &Vars) {
        self.env.append(vars);
    }

    pub fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read().unwrap()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read().unwrap()
    }

    pub fn outputs(&self) -> Vars {
        let outputs = &self.workflow().outputs;
        let vars = utils::fill_proc_vars(&self.env, outputs);
        vars
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
    }

    pub fn workflow_state(self: &Arc<Self>, event: &EventAction) -> WorkflowState {
        WorkflowState {
            pid: self.pid(),
            mid: self.workflow().id.clone(),
            event: event.clone(),
            state: self.state(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            outputs: self.outputs(),
            // proc: self.clone(),
        }
    }

    pub fn pid(&self) -> String {
        self.pid.clone()
    }

    pub fn workflow(&self) -> Arc<Workflow> {
        self.model.clone()
    }

    pub fn info(&self) -> ProcInfo {
        let workflow = self.workflow();
        ProcInfo {
            pid: self.pid.clone(),
            name: workflow.name.clone(),
            mid: workflow.id.clone(),
            state: self.state().into(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            // vars: self.vm().vars(),
        }
    }
    pub fn task(&self, tid: &str) -> Option<Arc<Task>> {
        self.tasks.read().unwrap().task_by_tid(tid)
    }

    pub fn find_next_tasks(&self, tid: &str) -> Vec<Arc<Task>> {
        let tasks = self.tasks.read().unwrap();
        tasks.find_next_tasks(tid)
    }

    pub fn node(&self, nid: &str) -> Option<Arc<Node>> {
        self.tree.node(nid)
    }

    pub fn tasks(&self) -> Vec<Arc<Task>> {
        let ttree = self.tasks.read().unwrap();
        ttree.tasks()
    }
    pub fn acts(&self) -> Vec<Arc<Act>> {
        let acts = &*self.acts.read().unwrap();
        acts.iter().map(|(_, act)| act.clone()).collect()
    }

    pub fn task_by_nid(&self, nid: &str) -> Vec<Arc<Task>> {
        self.tasks.read().unwrap().task_by_nid(nid)
    }

    pub fn last_task_by_nid(&self, nid: &str) -> Option<Arc<Task>> {
        self.tasks.read().unwrap().last_task_by_nid(nid)
    }

    pub fn create_context(
        self: &Arc<Self>,
        scher: &Arc<Scheduler>,
        task: &Arc<Task>,
    ) -> Arc<Context> {
        let ctx = Context::new(scher, &self, task);
        Arc::new(ctx)
    }

    pub fn act(&self, id: &str) -> Option<Arc<Act>> {
        self.acts.read().unwrap().get(id).cloned()
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            self.set_end_time(utils::time::time());
        } else if state.is_running() {
            self.set_start_time(utils::time::time());
        }
        *self.state.write().unwrap() = state;
    }

    pub(crate) fn set_start_time(&self, time: i64) {
        *self.start_time.write().unwrap() = time;
    }
    pub(crate) fn set_end_time(&self, time: i64) {
        *self.end_time.write().unwrap() = time;
    }

    #[instrument]
    pub fn do_ack(self: Arc<Self>, aid: &str, _scher: &Arc<Scheduler>) -> ActResult<ActionState> {
        let mut count = self.sync.lock().unwrap();
        let mut state = ActionState::begin();
        // todo: ack for message in the future
        state.end();
        *count += 1;

        Ok(state)
    }

    #[instrument]
    pub fn do_action(
        self: Arc<Self>,
        action: &Action,
        scher: &Arc<Scheduler>,
    ) -> ActResult<ActionState> {
        let mut count = self.sync.lock().unwrap();
        let mut state = ActionState::begin();
        let act = self.act(&action.aid).ok_or(ActError::Action(format!(
            "cannot find act by '{}'",
            action.aid
        )))?;

        let ctx = act.create_context(scher);
        act.exec(&ctx, action)?;

        let event = action.event();
        if event == EventAction::Complete
            || event == EventAction::Abort
            || event == EventAction::Error
            || event == EventAction::Skip
        {
            act.post(&ctx)?;
            act.next(&ctx)?;
        }

        state.end();
        *count += 1;
        Ok(state)
    }

    #[instrument]
    pub fn do_task(self: Arc<Self>, tid: &str, scher: &Arc<Scheduler>) {
        let mut count = self.sync.lock().unwrap();
        if let Some(task) = &self.task(tid) {
            if !task.state().is_completed() {
                let ctx = self.create_context(scher, task);
                task.exec(&ctx);
            }
        }
        *count += 1;
    }

    #[instrument]
    pub fn start(self: Arc<Self>, scher: &Arc<Scheduler>) {
        let mut count = self.sync.lock().unwrap();
        scher.cache().push_proc(&self);

        let tr = self.tree.clone();
        self.set_state(TaskState::Running);
        if let Some(root) = &tr.root {
            let task = self.create_task(root, None);
            scher.sched_task(&task);
        }
        *count += 1;
    }

    pub fn create_task(self: &Arc<Proc>, node: &Arc<Node>, prev: Option<Arc<Task>>) -> Arc<Task> {
        let task = Arc::new(Task::new(&self, &utils::shortid(), node.clone()));

        if let Some(prev) = prev {
            task.set_prev(Some(prev.tid.clone()));
            prev.push_back(&task.tid);
        }

        self.push_task(task.clone());

        task
    }

    pub fn push_task(&self, task: Arc<Task>) {
        let mut tasks = self.tasks.write().unwrap();
        tasks.push(task);
    }

    #[instrument]
    pub fn push_act(&self, act: &Arc<Act>) {
        let mut acts = self.acts.write().unwrap();
        acts.insert(act.id.clone(), act.clone());
    }
}
