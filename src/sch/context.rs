use super::{proc::Dispatcher, ActKind};
use crate::{
    env::VirtualMachine,
    event::{Action, EventAction, EventData, MessageKind},
    sch::{proc::Act, tree::NodeData, Node, Proc, Scheduler, Task},
    utils::{self, consts},
    ActError, ActResult, ActValue, ShareLock, TaskState,
};
use std::sync::{Arc, RwLock};
use tracing::debug;

#[derive(Clone)]
pub struct Context {
    pub scher: Arc<Scheduler>,
    pub proc: Arc<Proc>,
    pub task: Arc<Task>,
    pub action: ShareLock<Option<EventAction>>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("pid", &self.proc.pid())
            .field("tid", &self.task.tid)
            .field("action", &self.action())
            .finish()
    }
}

impl Context {
    fn init_vars(&self, task: &Task) {
        let vars = match &task.node.data {
            NodeData::Workflow(workflow) => workflow.env.clone(),
            NodeData::Job(job) => job.env.clone(),
            NodeData::Branch(branch) => branch.env.clone(),
            NodeData::Step(step) => step.env.clone(),
        };
        self.env().append(&vars);
    }

    pub fn new(scher: &Arc<Scheduler>, proc: &Arc<Proc>, task: &Arc<Task>) -> Self {
        let ctx = Context {
            scher: scher.clone(),
            proc: proc.clone(),
            action: Arc::new(RwLock::new(None)),
            task: task.clone(),
        };

        ctx
    }

    pub fn prepare(&self) {
        // bind current context to env
        self.env().bind_context(self);
        self.init_vars(&self.task);
    }

    pub fn set_action_vars(&self, action: &Action) -> ActResult<()> {
        *self.action.write().unwrap() = Some(action.event.as_str().into());
        self.env().append(&action.options);

        Ok(())
    }

    pub fn run(&self, script: &str) -> ActResult<bool> {
        self.task.env.run(script)
    }

    pub fn eval(&self, expr: &str) -> ActResult<bool> {
        self.task.env.eval(expr)
    }

    pub fn eval_with<T: rhai::Variant + Clone>(&self, expr: &str) -> ActResult<T> {
        self.task.env.eval(expr)
    }

    pub fn var(&self, name: &str) -> Option<ActValue> {
        self.env().get(name)
    }

    pub(in crate::sch) fn env(&self) -> &VirtualMachine {
        &self.task.env
    }

    #[allow(unused)]
    pub(in crate::sch) fn action(&self) -> Option<EventAction> {
        self.action.read().unwrap().clone()
    }

    pub fn sched_task(&self, node: &Arc<Node>) {
        let task = self.proc.create_task(&node, Some(self.task.clone()));
        self.scher.sched_task(&task);
    }

    /// redo the task and dispatch directly
    pub fn redo_task(&self, task: &Arc<Task>) -> ActResult<()> {
        let vars = &task.env.vars();
        let task = self.proc.create_task(&task.node, Some(self.task.clone()));
        for key in consts::ACT_VARS {
            if let Some(v) = vars.get(key) {
                task.env.set(key, v.clone());
            }
        }
        task.set_state(TaskState::Running);
        self.dispatch_task(&task, EventAction::Create);

        let ctx = task.create_context(&self.scher);
        let dispatcher = Dispatcher::new(&ctx);
        dispatcher.redo()
    }

    pub fn back_task(&self, task: &Arc<Task>, aid: &str) -> ActResult<()> {
        for act in task.acts().iter().filter(|act| act.kind == ActKind::User) {
            if act.state().is_completed() {
                continue;
            }
            if act.id == aid {
                act.set_state(TaskState::Backed);
                self.dispatch_act(act, EventAction::Back);
            } else {
                act.set_state(TaskState::Skip);
                self.dispatch_act(act, EventAction::Skip);
            }
        }
        task.set_state(TaskState::Backed);
        self.dispatch_task(task, EventAction::Back);

        Ok(())
    }

    pub fn abort_task(&self, task: &Arc<Task>, aid: &str) -> ActResult<()> {
        // abort all task's act
        for act in task.acts().iter().filter(|act| act.kind == ActKind::User) {
            if act.id == aid {
                act.set_state(TaskState::Abort);
                self.dispatch_act(act, EventAction::Abort);
            } else {
                act.set_state(TaskState::Skip);
                self.dispatch_act(act, EventAction::Skip);
            }
        }

        // abort all running task
        let ctx = self;
        let mut parent = task.parent(ctx);
        while let Some(task) = parent {
            let proc = ctx.proc.clone();
            let ctx = proc.create_context(&ctx.scher, &task);
            ctx.task.set_state(TaskState::Abort);
            ctx.dispatch_task(&ctx.task, EventAction::Abort);

            for tid in task.children() {
                if let Some(task) = ctx.proc.task(&tid) {
                    if task.state().is_waiting() || task.state().is_running() {
                        task.set_state(TaskState::Abort);
                        ctx.dispatch_task(&task, EventAction::Abort);
                    }
                }
            }

            parent = task.parent(&ctx);
        }
        Ok(())
    }

    pub fn redo_act(&self, act: &Arc<Act>) -> ActResult<()> {
        act.set_state(TaskState::Cancelled);
        self.dispatch_act(&act, EventAction::Cancel);

        // create a new act
        let act = self.task.push_act(act.kind.clone(), &act.vars);
        self.dispatch_act(&act, EventAction::Create);

        Ok(())
    }

    /// undo task
    pub fn undo_task(&self, task: &Arc<Task>) -> ActResult<()> {
        if task.state().is_completed() {
            return Err(ActError::Action(format!(
                "task('{}') is not allowed to cancel",
                task.tid
            )));
        }

        // cancel all of the task's acts
        for act in task.acts() {
            act.set_state(TaskState::Cancelled);
            self.dispatch_act(&act, EventAction::Cancel);
        }
        task.set_state(TaskState::Cancelled);
        self.dispatch_task(&task, EventAction::Cancel);

        Ok(())
    }

    pub fn dispatch_task(&self, task: &Task, action: EventAction) {
        debug!("ctx::dispatch, task={:?} action={:?}", task, action);

        let data = EventData {
            pid: self.proc.pid(),
            event: action.clone(),
        };

        // on workflow start
        if let NodeData::Workflow(_) = &task.node.data {
            if action == EventAction::Create {
                self.proc.set_state(TaskState::Running);
                self.scher.emitter().dispatch_proc_event(&self.proc, &data);
            }
        }
        match &task.node.data {
            NodeData::Job(job) => {
                // let mut outputs = Vars::new();
                if action == EventAction::Complete {
                    let outputs = utils::fill_vars(&self.task.env, &job.outputs);
                    self.env().output(&outputs);

                    // re-assign the vars
                    // data.vars = self.env().vars();
                }
            }
            _ => {
                // do nothing
            }
        }

        // exec on events from model config
        self.task.on_event(&action.to_string(), self);
        self.scher.emitter().dispatch_task_event(task, &data);

        // on workflow complete
        if let NodeData::Workflow(_) = &task.node.data {
            if action != EventAction::Create {
                self.proc.set_state(task.state());
                self.scher.emitter().dispatch_proc_event(&self.proc, &data);
            }
        }

        let state = self.task.state();
        if state.is_error() {
            let mut parent = self.task.parent(self);
            while let Some(task) = &parent {
                task.set_state(state.clone());
                self.scher.emitter().dispatch_task_event(task, &data);

                parent = task.parent(self);
            }

            // dispatch workflow event
            self.proc.set_state(state.clone());
            self.scher.emitter().dispatch_proc_event(&self.proc, &data);
        }
    }

    pub fn dispatch_act(&self, act: &Arc<Act>, action: EventAction) {
        if !act.active() {
            act.set_active(true);
        }
        if act.state().is_none() {
            act.set_state(TaskState::WaitingEvent);
        }

        if self.task.state().is_running() {
            self.task.set_state(TaskState::WaitingEvent);
        }

        let mut vars = self.task.vars();
        for (key, value) in act.vars() {
            vars.entry(key.to_string())
                .and_modify(|item| *item = value.clone())
                .or_insert(value.clone());
        }

        let edata = EventData {
            pid: self.proc.pid(),
            event: action.clone(),
        };
        act.on_event(&edata.event.to_string(), self);
        if !self.task.state().is_error() {
            self.scher.emitter().dispatch_act_event(act, &edata);
        }
    }

    pub fn dispatch_message(&self, key: &str) {
        let mut message = self.task.create_message(&EventAction::Create);
        message.key = Some(key.to_string());
        message.kind = MessageKind::Notice;

        self.scher.emitter().dispatch_message(&message);
    }
}
