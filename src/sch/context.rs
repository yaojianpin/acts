use crate::{
    event::{Action, ActionState, EventAction, Model},
    sch::{tree::NodeContent, Node, Proc, Scheduler, Task},
    utils::{self, consts, shortid},
    Act, ActError, Error, Message, ModelBase, Msg, NodeKind, Result, ShareLock, TaskState, Vars,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tracing::debug;

use super::ActTask;

#[derive(Clone)]
pub struct Context {
    pub scher: Arc<Scheduler>,
    pub proc: Arc<Proc>,
    pub task: Arc<Task>,
    pub action: ShareLock<Option<EventAction>>,
    pub err: ShareLock<Option<Error>>,
    pub vars: ShareLock<Vars>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("pid", &self.proc.id())
            .field("tid", &self.task.id)
            .field("action", &self.action())
            .finish()
    }
}

impl Context {
    fn init_vars(&self, task: &Task) {
        let inputs = task.inputs();
        debug!("init_vars: {inputs}");
        self.task.env().set_env(&inputs);
    }

    pub fn new(scher: &Arc<Scheduler>, proc: &Arc<Proc>, task: &Arc<Task>) -> Self {
        let ctx = Context {
            scher: scher.clone(),
            proc: proc.clone(),
            action: Arc::new(RwLock::new(None)),
            task: task.clone(),
            err: Arc::new(RwLock::new(task.state().as_err())),
            vars: Arc::new(RwLock::new(Vars::new())),
        };

        ctx
    }

    pub fn prepare(&self) {
        // bind current context to env
        self.task.env().bind_context(self);
        self.init_vars(&self.task);
    }

    pub fn set_action(&self, action: &Action) -> Result<()> {
        *self.action.write().unwrap() = Some(EventAction::parse(action.event.as_str())?);

        // set the action options to the context
        let mut vars = self.vars.write().unwrap();
        for (name, v) in &action.options {
            vars.entry(name.to_string())
                .and_modify(|i| *i = v.clone())
                .or_insert(v.clone());
        }

        Ok(())
    }

    pub fn err(&self) -> Option<Error> {
        self.err.read().unwrap().clone()
    }
    pub fn set_err(&self, err: &Error) {
        *self.err.write().unwrap() = Some(err.clone());
    }

    pub fn vars(&self) -> Vars {
        self.vars.read().unwrap().clone()
    }

    pub fn set_var<T>(&self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        self.vars.write().unwrap().set(name, value);
    }

    pub fn get_var<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        self.vars.read().unwrap().get::<T>(name)
    }

    pub fn run(&self, script: &str) -> Result<bool> {
        self.task.env().run(script)
    }

    pub fn eval(&self, expr: &str) -> Result<bool> {
        self.task.env().eval(expr)
    }

    pub fn eval_with<T: rhai::Variant + Clone>(&self, expr: &str) -> Result<T> {
        self.task.env().eval(expr)
    }

    #[allow(unused)]
    pub(in crate::sch) fn action(&self) -> Option<EventAction> {
        self.action.read().unwrap().clone()
    }

    pub fn sched_task(&self, node: &Arc<Node>) {
        let task = self.proc.create_task(&node, Some(self.task.clone()));
        self.scher.push(&task);
    }

    pub fn append_act(&self, act: &Act) -> Result<Arc<Node>> {
        debug!("append_act: {act:?}  {:?}", self.task);
        let mut task = self.task.clone();
        if self.task.is_kind(NodeKind::Act) {
            let is_package_act = self.task.is_act(consts::ACT_TYPE_PACK);

            // not package act or completed package
            if !is_package_act || (is_package_act && self.task.state().is_completed()) {
                // find its parent to append task
                while let Some(parent) = task.parent() {
                    if parent.is_kind(NodeKind::Step) || parent.is_act(consts::ACT_TYPE_PACK) {
                        task = parent;
                        break;
                    }
                    task = parent;
                }
            }
        }

        let mut id = act.id().to_string();
        if id.is_empty() {
            id = shortid();
        }

        let node = Arc::new(Node::new(
            &id,
            NodeContent::Act(act.clone()),
            task.node.level + 1,
        ));
        node.set_parent(&task.node);

        if task.state().is_running() {
            let task = self.proc.create_task(&node, Some(task));
            self.scher.push(&task);
        }

        Ok(node)
    }

    /// redo the task and dispatch directly
    pub fn redo_task(&self, task: &Arc<Task>) -> Result<()> {
        if let Some(prev) = task.prev() {
            if let Some(prev_task) = self.task.proc().task(&prev) {
                let task = self.proc.create_task(&task.node, Some(prev_task));
                self.scher.push(&task);
            }
        }

        Ok(())
    }

    pub fn back_task(&self, task: &Arc<Task>, paths: &Vec<Arc<Task>>) -> Result<()> {
        for task in task.siblings().iter() {
            if task.state().is_completed() {
                continue;
            }
            task.set_action_state(ActionState::Skipped);
            self.emit_task(task);
        }

        task.set_action_state(ActionState::Backed);
        self.emit_task(task);

        for p in paths {
            if p.is_kind(NodeKind::Act) {
                p.set_action_state(ActionState::Backed);
                self.emit_task(&p);
            }

            if p.state().is_running() {
                p.set_action_state(ActionState::Completed);
                self.emit_task(&p);
            } else if p.state().is_pending() {
                p.set_action_state(ActionState::Skipped);
                self.emit_task(&p);
            }
        }

        Ok(())
    }

    pub fn abort_task(&self, task: &Arc<Task>) -> Result<()> {
        // abort all task's acts
        for task in task.siblings().iter() {
            if task.state().is_completed() {
                continue;
            }
            task.set_action_state(ActionState::Skipped);
            self.emit_task(task);
        }

        task.set_action_state(ActionState::Aborted);
        self.emit_task(task);

        // abort all running task
        let ctx = self;
        let mut parent = task.parent();
        while let Some(task) = parent {
            let ctx = ctx.proc.create_context(&ctx.scher, &task);
            ctx.task.set_action_state(ActionState::Aborted);
            ctx.emit_task(&ctx.task);

            for t in task.children() {
                if t.state().is_pending() {
                    t.set_action_state(ActionState::Skipped);
                    ctx.emit_task(&t);
                } else if t.state().is_running() {
                    t.set_action_state(ActionState::Aborted);
                    ctx.emit_task(&t);
                }
            }

            parent = task.parent();
        }
        Ok(())
    }

    /// undo task
    /// the undo task is a step task, set the task as completed and set the children acts as cancelled
    pub fn undo_task(&self, task: &Arc<Task>) -> Result<()> {
        if task.state().is_completed() {
            return Err(ActError::Action(format!(
                "task('{}') is not allowed to cancel",
                task.id
            )));
        }

        // cancel all of the task's children
        let mut children = task.children();
        while children.len() > 0 {
            let mut nexts = Vec::new();
            for t in &children {
                if t.state().is_completed() {
                    continue;
                }
                t.set_action_state(ActionState::Cancelled);
                self.emit_task(&t);
                nexts.extend_from_slice(&t.children());
            }

            children = nexts;
        }
        task.set_action_state(ActionState::Completed);
        self.emit_task(&task);

        Ok(())
    }

    pub fn emit_error(&self) -> Result<()> {
        let state = self.task.state();
        if state.is_error() {
            self.emit_task(&self.task);

            // after emitting, re-check the task state
            if self.task.state().is_error() {
                if let Some(parent) = self.task.parent() {
                    let ctx = parent.create_context(&self.scher);
                    ctx.set_err(&state.as_err().unwrap_or_default());
                    return parent.error(&ctx);
                }
            }
        }

        Ok(())
    }

    pub fn emit_task(&self, task: &Arc<Task>) {
        debug!("ctx::emit_task, task={:?}", task);
        // on workflow start
        if let NodeContent::Workflow(_) = &task.node.content {
            if task.action_state() == ActionState::Created {
                self.proc.set_state(TaskState::Running);
                self.scher.emitter().emit_proc_event(&self.proc);
            }
        }

        if task.action_state() == ActionState::Completed
            || task.action_state() == ActionState::Submitted
        {
            let outputs = task.outputs();
            if outputs.len() > 0 {
                if let Some(parent) = task.parent() {
                    parent.env().set_env(&outputs);
                }
            }
        }

        self.scher.emitter().emit_task_event(task);

        // on workflow complete
        if let NodeContent::Workflow(_) = &task.node.content {
            if task.action_state() != ActionState::Created {
                self.proc.set_state(task.state());
                self.scher.emitter().emit_proc_event(&self.proc);
            }
        }
    }

    pub fn emit_message(&self, msg: &Msg) {
        debug!("emit_message: {:?}", msg);
        let workflow = self.proc.model();

        let inputs = utils::fill_inputs(&self.task.env(), &msg.inputs);
        // if there is no key, use id instead
        let mut key = &msg.key;
        if key.is_empty() {
            key = &msg.id;
        }

        let msg = Message {
            id: utils::shortid(),
            r#type: consts::ACT_TYPE_MSG.to_string(),
            source: self.task.node.kind().to_string(),
            state: self.task.action_state().to_string(),
            proc_id: self.task.proc_id.clone(),
            key: key.to_string(),
            name: msg.name.clone(),

            model: Model {
                id: workflow.id.clone(),
                name: workflow.name.to_string(),
                tag: workflow.tag.to_string(),
            },

            tag: msg.tag.to_string(),
            inputs,
            ..Default::default()
        };
        self.scher.emitter().emit_message(&msg);
    }
}
