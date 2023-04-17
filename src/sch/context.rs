use crate::{
    env::VirtualMachine,
    event::{consts, ActionOptions, EventAction, EventData, UserMessage},
    sch::{tree::NodeData, Proc, Task},
    utils, ActResult, ActValue, ShareLock, TaskState, Vars,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tracing::debug;

use super::{Node, Scheduler};

#[derive(Clone)]
pub struct Context {
    pub scher: Arc<Scheduler>,
    pub proc: Arc<Proc>,
    pub task: Arc<Task>,
    uid: ShareLock<Option<String>>,
    action: ShareLock<Option<String>>,
    options: ShareLock<ActionOptions>,
}

impl Context {
    fn append_vars(&self, vars: &Vars) {
        debug!("append_vars vars={:?}", vars);
        let env = &mut self.proc.vm.vars.write().unwrap();
        for (name, v) in vars {
            env.entry(name.to_string())
                .and_modify(|i| *i = v.clone())
                .or_insert(v.clone());
        }
    }

    fn init_vars(&self, task: &Task) {
        let mut vars = Vars::new();
        match &task.node.data {
            NodeData::Workflow(workflow) => vars = workflow.env.clone(),
            NodeData::Job(job) => vars = job.env.clone(),
            NodeData::Branch(branch) => vars = branch.env.clone(),
            NodeData::Step(step) => vars = step.env.clone(),
            NodeData::Act(_act) => {}
        }

        self.append_vars(&vars);
    }

    pub fn new(scher: &Arc<Scheduler>, proc: &Arc<Proc>, task: &Arc<Task>) -> Self {
        let ctx = Context {
            scher: scher.clone(),
            proc: proc.clone(),
            uid: Arc::new(RwLock::new(None)),
            action: Arc::new(RwLock::new(None)),
            options: Arc::new(RwLock::new(ActionOptions::default())),
            task: task.clone(),
        };

        ctx
    }

    pub fn prepare(&self) {
        self.init_vars(&self.task);
    }

    pub fn set_message(&self, msg: &UserMessage) {
        *self.uid.write().unwrap() = Some(msg.uid.clone());
        *self.action.write().unwrap() = Some(msg.action.clone());

        if let Some(options) = &msg.options {
            self.append_vars(&options.vars);
            *self.options.write().unwrap() = options.clone();
        }
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

    pub(in crate::sch) fn uid(&self) -> Option<String> {
        self.uid.read().unwrap().clone()
    }

    #[allow(unused)]
    pub(in crate::sch) fn action(&self) -> Option<String> {
        self.action.read().unwrap().clone()
    }

    pub(in crate::sch) fn options(&self) -> ActionOptions {
        let ret = self.options.read().unwrap();
        ret.clone()
    }

    pub fn sched_task(&self, node: &Arc<Node>) {
        let task = self.proc.create_task(&node, Some(self.task.clone()));
        self.scher.sched_task(&task);
    }

    pub fn dispatch(&self, task: &Task, action: EventAction) {
        debug!("ctx::dispatch, task={:?} action={:?}", task, action);
        let mut on_event = HashMap::new();
        let mut data = EventData {
            pid: self.proc.pid(),
            state: task.state(),
            action: action.clone(),
            vars: self.vm().vars(),
        };

        // on workflow start
        if let NodeData::Workflow(_) = &task.node.data {
            if action == EventAction::Create {
                self.proc.set_state(&TaskState::Running);
                self.proc.set_start_time(utils::time::time());

                self.scher.emitter().dispatch_proc_event(&self.proc, &data);
            }
        }
        match &task.node.data {
            NodeData::Job(job) => {
                // let mut outputs = Vars::new();
                if action == EventAction::Next {
                    let outputs = utils::fill_vars(&self.proc.vm, &job.outputs);
                    self.append_vars(&outputs);

                    // re-assign the vars
                    data.vars = self.vm().vars();
                }
            }
            NodeData::Step(step) => {
                on_event = step.on.clone();
            }
            NodeData::Act(act) => {
                if let Some(step) = act.parent(self) {
                    match &step.node.data {
                        NodeData::Step(step) => {
                            if let Some(sub) = &step.subject {
                                on_event = sub.on.clone();
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                // do nothing
            }
        }

        // dispatch task event
        self.scher
            .emitter()
            .dispatch_task_event(&self.proc, task, &data);

        // exec events from model config
        let evt_name = self.get_action_name(&action);
        if let Some(event) = on_event.get(&evt_name) {
            if event.is_string() {
                let ret = self.run(event.as_str().unwrap());
                if ret.is_err() {
                    task.set_state(&TaskState::Fail(ret.err().unwrap().into()));
                }
            }
        }

        // on workflow complete
        if let NodeData::Workflow(_) = &task.node.data {
            if action != EventAction::Create {
                self.proc.set_state(&task.state());
                self.proc.set_end_time(utils::time::time());

                self.scher.emitter().dispatch_proc_event(&self.proc, &data);
            }
        }
        // if task.state() == TaskState::WaitingEvent {
        //     let tid = task.tid();
        //     let uid = task.uid();
        //     let message = self.proc.make_message(&tid, uid, self.vars(task));
        //     self.proc.scher.emitter().dispatch_message(&message);
        // }
    }

    fn get_action_name(&self, action: &EventAction) -> String {
        match action {
            EventAction::Create => consts::EVT_INIT.to_string(),
            EventAction::Next => consts::EVT_NEXT.to_string(),
            EventAction::Back => consts::EVT_BACK.to_string(),
            EventAction::Cancel => consts::EVT_CANCEL.to_string(),
            EventAction::Abort => consts::EVT_ABORT.to_string(),
            EventAction::Submit => consts::EVT_SUBMIT.to_string(),
            EventAction::Error => consts::EVT_ERROR.to_string(),
            EventAction::Skip => consts::EVT_SKIP.to_string(),
            EventAction::Custom(name) => name.to_string(),
        }
    }
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("pid", &self.proc.pid())
            .field("tid", &self.task.tid)
            .field("uid", &self.uid())
            .field("action", &self.action())
            .finish()
    }
}
