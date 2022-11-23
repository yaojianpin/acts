use crate::{
    debug,
    env::VirtualMachine,
    sch::{
        consts,
        event::{EventAction, EventData, UserData},
        ActId, ActState, Proc, Task,
    },
    utils, ActResult, ActValue, Job, Message, ShareLock, TaskState, Vars, Workflow,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Clone)]
pub struct Context {
    pub proc: Arc<Proc>,
    pub user_data: ShareLock<UserData>,
    pub job: ShareLock<Option<Job>>,

    pub tid: Arc<RwLock<Option<String>>>,

    sync: Arc<Mutex<i32>>,
}

impl Context {
    fn append_vars(&self, vars: &Vars) {
        let env = &mut self.proc.vm.vars.write().unwrap();
        for (name, v) in vars {
            env.entry(name.to_string())
                .and_modify(|i| *i = v.clone())
                .or_insert(v.clone());
        }
    }

    fn init_vars(&self, task: &Task) {
        let mut vars = Vars::new();
        match task {
            Task::Workflow(_, workflow) => vars = workflow.env.clone(),
            Task::Job(_, job) => vars = job.env.clone(),
            Task::Branch(_, branch) => vars = branch.env.clone(),
            Task::Step(_, step) => vars = step.env.clone(),
            Task::Act(_, _act) => {}
        }

        self.append_vars(&vars);
        debug!("vars={:?}", vars);
    }

    pub fn new(proc: &Proc) -> Self {
        let ctx = Context {
            proc: Arc::new(proc.clone()),
            user_data: Arc::new(RwLock::new(UserData::default())),
            job: Arc::new(RwLock::new(None)),
            tid: Arc::new(RwLock::new(None)),
            sync: Arc::new(Mutex::new(0)),
        };

        ctx
    }

    pub fn prepare(&self, task: &Task) {
        self.init_vars(task);

        //self.push_task(exec);
        self.set_task(task);
        if task.state() == TaskState::None {
            self.dispatch(task, EventAction::Create);
        }

        let is_pass = task.check_pass(self);
        if is_pass {
            task.set_state(&TaskState::Running);
        } else {
            task.set_state(&TaskState::WaitingEvent);
        }
    }

    pub fn post(&self, task: &Task) {
        let mut count = self.sync.lock().unwrap();
        match task {
            Task::Workflow(_, workflow) => {
                if workflow.is_finished() {
                    let outputs = &workflow.outputs;
                    let vars = utils::fill_vars(&self.proc.vm, outputs);
                    workflow.set_outputs(vars);

                    if workflow.state() == TaskState::Running {
                        workflow.set_state(&TaskState::Success);
                    }
                    self.proc.complete();
                }
            }
            Task::Job(_, job) => {
                if task.state() == TaskState::Running {
                    task.set_state(&TaskState::Success);
                }
                let outputs = &job.outputs;
                let vars = utils::fill_vars(&self.proc.vm, outputs);
                self.append_vars(&vars);
            }
            _ => {
                if task.state() == TaskState::Running {
                    task.set_state(&TaskState::Success);
                }
            }
        }

        self.dispatch(task, EventAction::Next);

        *count += 1;
    }

    pub fn set_message(&self, msg: &Message) {
        if let Some(user_data) = &msg.data {
            let vars = user_data.vars.clone();
            if !vars.is_empty() {
                self.append_vars(&vars);
            }
            *self.user_data.write().unwrap() = user_data.clone();
        }
    }

    pub fn job(&self) -> Option<Job> {
        self.job.read().unwrap().clone()
    }

    pub fn workflow(&self) -> Arc<Workflow> {
        self.proc.workflow()
    }

    pub fn user_data(&self) -> UserData {
        self.user_data.read().unwrap().clone()
    }

    pub fn task(&self) -> Option<Task> {
        if let Some(tid) = &*self.tid.read().unwrap() {
            if let Some(node) = self.proc.tree.node(tid) {
                return Some(node.data());
            }
        }

        None
    }

    pub fn run(&self, script: &str) -> ActResult<bool> {
        self.proc.vm.run(script)
    }

    pub fn eval(&self, expr: &str) -> ActResult<bool> {
        self.proc.vm.eval(expr)
    }

    pub fn eval_with<T: rhai::Variant + Clone>(&self, expr: &str) -> ActResult<T> {
        self.proc.vm.eval(expr)
    }

    pub fn var(&self, name: &str) -> Option<ActValue> {
        self.vm().get(name)
    }

    pub(in crate::sch) fn vm(&self) -> &VirtualMachine {
        &self.proc.vm
    }

    pub(in crate::sch) fn user(&self) -> Option<String> {
        let user = self.user_data().user;
        if user.is_empty() {
            return None;
        }

        return Some(user);
    }

    fn set_task(&self, task: &Task) {
        *self.tid.write().unwrap() = Some(task.tid());

        match task {
            Task::Job(_, job) => {
                *self.job.write().unwrap() = Some(job.clone());
            }
            _ => {}
        }
    }

    pub fn send_message(&self, user: &str, task: &Task) {
        match task.state() {
            TaskState::Abort(err) | TaskState::Fail(err) => {
                let state = &TaskState::Fail(err);
                self.job().unwrap().set_state(state);

                let workflow = self.workflow();
                workflow.set_state(state);

                self.proc.scher.evt().disp_workflow(&workflow);
                self.proc.complete();
            }
            TaskState::WaitingEvent => {
                let tid = task.tid();
                let message = self.proc.make_message(&tid, user);
                self.proc.scher.evt().disp_message(&message);
            }
            _ => {}
        }
    }

    pub fn dispatch(&self, task: &Task, action: EventAction) {
        let mut user = "".to_string();
        let data = EventData {
            pid: self.proc.pid(),
            state: task.state(),
            action: action.clone(),
            vars: self.vm().vars(),
        };
        self.proc.scher.evt().disp_task(task, &data);
        let mut on = HashMap::new();
        match task {
            Task::Workflow(_, workflow) => {
                self.proc.scher.evt().disp_workflow(workflow);
            }
            Task::Job(_, job) => {
                self.proc.scher.evt().disp_job(job);
            }
            Task::Branch(_, _branch) => {}
            Task::Step(_, step) => {
                self.proc.scher.evt().disp_step(step);
                on = step.on.clone();
            }
            Task::Act(_, act) => {
                user = act.owner.clone();
                self.proc.scher.evt().disp_act(act);
                if let Some(step) = act.parent(self) {
                    match step {
                        Task::Step(_, step) => {
                            if let Some(sub) = step.subject {
                                on = sub.on.clone();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let evt = self.get_action_name(&action);
        if let Some(event) = on.get(&evt) {
            if event.is_string() {
                let ret = self.run(event.as_str().unwrap());
                if ret.is_err() {
                    task.set_state(&TaskState::Fail(ret.err().unwrap().into()));
                }
            }
        }

        self.send_message(&user, task);
    }

    fn get_action_name(&self, action: &EventAction) -> String {
        match action {
            EventAction::Create => consts::EVT_INIT.to_string(),
            EventAction::Next => consts::EVT_NEXT.to_string(),
            EventAction::Back => consts::EVT_BACK.to_string(),
            EventAction::Cancel => consts::EVT_CANCEL.to_string(),
            EventAction::Error => consts::EVT_ERROR.to_string(),
        }
    }
}
