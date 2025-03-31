use super::{ActTask, Runtime};
use crate::{
    event::{Action, Model},
    sch::{tree::NodeContent, Node, Proc, Task},
    utils::{self, consts, shortid},
    Act, ActError, Message, MessageState, NodeKind, Result, TaskState, Vars,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{cell::RefCell, sync::Arc};
use tracing::debug;

tokio::task_local! {
    static CONTEXT: Context;
}

#[derive(Clone)]
pub struct Context {
    // pub scher: Arc<Scheduler>,
    // pub env: Arc<Enviroment>,
    pub runtime: Arc<Runtime>,
    pub proc: Arc<Proc>,
    task: RefCell<Arc<Task>>,
    action: RefCell<Option<Action>>,
    vars: RefCell<Vars>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("pid", &self.proc.id())
            .field("tid", &self.task().id)
            .field("action", &self.action())
            .finish()
    }
}

impl Context {
    fn init_vars(&self, task: &Arc<Task>) {
        let inputs = task.inputs();
        debug!("init_vars: {inputs}");
        self.task().set_data_with(|data| {
            for (ref k, v) in &inputs {
                data.set(k, v.clone());
            }
        });
    }

    pub fn new(proc: &Arc<Proc>, task: &Arc<Task>) -> Self {
        let ctx = Context {
            runtime: task.runtime().clone(),
            proc: proc.clone(),
            action: RefCell::new(None),
            task: RefCell::new(task.clone()),
            vars: RefCell::new(Vars::new()),
        };

        ctx
    }

    pub fn scope<T, F: Fn() -> T>(ctx: Context, f: F) -> T {
        if Context::current().is_ok() {
            f()
        } else {
            CONTEXT.sync_scope(ctx, f)
        }
    }

    pub fn with<T, F: Fn(&Context) -> T>(f: F) -> T {
        CONTEXT.with(|ctx| f(ctx))
    }

    pub fn current() -> Result<Context> {
        CONTEXT
            .try_with(Clone::clone)
            .map_err(|e| ActError::Runtime(e.to_string()))
    }

    pub fn set_task(&self, task: &Arc<Task>) {
        if self.task.borrow().id != task.id {
            *self.task.borrow_mut() = task.clone();
        }
    }

    pub fn task(&self) -> Arc<Task> {
        self.task.borrow().clone()
    }

    pub fn prepare(&self) {
        self.init_vars(&self.task());
    }

    pub fn set_action(&self, action: &Action) -> Result<()> {
        *self.action.borrow_mut() = Some(action.clone());

        // set the action options to the context
        let mut vars = self.vars.borrow_mut();
        for (name, v) in &action.options {
            vars.entry(name.to_string())
                .and_modify(|i| *i = v.clone())
                .or_insert(v.clone());
        }

        Ok(())
    }

    pub fn vars(&self) -> Vars {
        self.vars.borrow().clone()
    }

    pub fn set_env<T>(&self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        // in context, the global env is not writable
        // just set the value to local env of the proc
        self.proc.with_env_local_mut(|data| {
            data.set(name, value);
        });
    }

    pub fn get_env<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        // find the env from env local firstly
        if let Some(v) = self.proc.with_env_local(|vars| vars.get(name)) {
            return Some(v);
        }

        // then get the value from global env
        if let Some(v) = self.runtime.env().get(name) {
            return Some(v);
        }
        None
    }

    pub fn set_var<T>(&self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        self.vars.borrow_mut().set(name, value);
    }

    pub fn get_var<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        self.vars.borrow().get::<T>(name)
    }

    pub fn eval<T: DeserializeOwned + Serialize>(&self, expr: &str) -> Result<T> {
        Context::scope(self.clone(), || self.runtime.env().eval::<T>(expr))
    }

    #[allow(unused)]
    pub(in crate::sch) fn action(&self) -> Option<Action> {
        self.action.borrow().clone()
    }

    pub fn sched_task(&self, node: &Arc<Node>) {
        debug!("sched_task: {}", node.to_string());
        let task = self.proc.create_task(node, Some(self.task()));
        self.runtime.push(&task);
    }

    pub fn append_act(&self, act: &Act) -> Result<Arc<Node>> {
        debug!("append_act: {act:?}  {:?}", self.task);
        let mut task = self.task();
        if task.is_kind(NodeKind::Act) {
            let is_package_act = task.is_act(consts::ACT_TYPE_BLOCK);

            // not package act or completed package
            if !is_package_act || task.state().is_completed() {
                // find its parent to append task
                while let Some(parent) = task.parent() {
                    if parent.is_kind(NodeKind::Step) || parent.is_act(consts::ACT_TYPE_BLOCK) {
                        task = parent;
                        break;
                    }
                    task = parent;
                }
            }
        }

        let mut id = act.id.to_string();
        if id.is_empty() {
            id = shortid();
        }

        let node = Arc::new(Node::new(
            &id,
            NodeContent::Act(act.clone()),
            task.node().level + 1,
        ));
        // node.set_parent(task.node());
        if task.state().is_ready() || task.state().is_running() {
            let task = self.proc.create_task(&node, Some(task));
            self.runtime.push(&task);
        }

        Ok(node)
    }

    /// redo the task and dispatch directly
    pub fn redo_task(&self, task: &Arc<Task>) -> Result<()> {
        if let Some(prev) = task.prev() {
            if let Some(prev_task) = self.proc.task(&prev) {
                let task = self.proc.create_task(task.node(), Some(prev_task));
                self.runtime.push(&task);
            }
        }

        Ok(())
    }

    pub fn back_task(&self, task: &Arc<Task>, paths: &Vec<Arc<Task>>) -> Result<()> {
        for task in task.siblings().iter() {
            if task.state().is_completed() {
                continue;
            }
            task.set_state(TaskState::Skipped);
            self.emit_task(task)?;
        }

        task.set_state(TaskState::Backed);
        self.emit_task(task)?;

        // find parent util to the step task and marks it as backed
        if task.is_kind(NodeKind::Act) {
            let mut parent = task.parent();
            while let Some(p) = parent {
                if p.is_kind(NodeKind::Step) || p.is_kind(NodeKind::Act) {
                    p.set_state(TaskState::Backed);
                    self.emit_task(&p)?;
                    break;
                }
                parent = p.parent();
            }
        }

        // marks the state in the paths
        for p in paths {
            if p.state().is_running() {
                p.set_state(TaskState::Completed);
                self.emit_task(p)?;
            } else if p.state().is_pending() {
                p.set_state(TaskState::Skipped);
                self.emit_task(p)?;
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
            task.set_state(TaskState::Skipped);
            self.emit_task(task)?;
        }

        task.set_state(TaskState::Aborted);
        self.emit_task(task)?;

        // abort all running task
        let ctx = self;
        let mut parent = task.parent();
        while let Some(task) = parent {
            task.set_state(TaskState::Aborted);
            ctx.set_task(&task);
            ctx.emit_task(&ctx.task())?;

            for t in task.children() {
                if t.state().is_pending() {
                    t.set_state(TaskState::Skipped);
                    ctx.emit_task(&t)?;
                } else if t.state().is_running() {
                    t.set_state(TaskState::Aborted);
                    ctx.emit_task(&t)?;
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
        while !children.is_empty() {
            let mut nexts = Vec::new();
            for t in &children {
                if t.state().is_completed() {
                    continue;
                }
                t.set_state(TaskState::Cancelled);
                self.emit_task(t)?;
                nexts.extend_from_slice(&t.children());
            }

            children = nexts;
        }
        task.set_state(TaskState::Completed);
        self.emit_task(task)?;

        Ok(())
    }

    pub fn emit_error(&self) -> Result<()> {
        let task = self.task();
        if task.state().is_error() {
            self.emit_task(&task)?;

            // after emitting, re-check the task state
            if task.state().is_error() {
                if let Some(err) = task.err() {
                    if let Some(parent) = task.parent() {
                        parent.set_err(&err);
                        return parent.error(self);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn emit_task(&self, task: &Arc<Task>) -> Result<()> {
        debug!("ctx::emit_task, task={:?}", task);

        // on workflow start
        if let NodeContent::Workflow(_) = &task.node().content {
            if task.state().is_created() {
                if self.proc.state().is_none() {
                    self.proc.set_state(TaskState::Running);
                }
                self.runtime.scher().emit_proc_event(&self.proc);
            }
        }

        self.runtime.scher().emit_task_event(task)?;

        // on workflow complete
        if let NodeContent::Workflow(_) = &task.node().content {
            if task.state().is_completed() {
                self.proc.set_state(task.state());
                if let Some(err) = task.err() {
                    self.proc.set_err(&err);
                }
                self.runtime.scher().emit_proc_event(&self.proc);
            }
        }

        Ok(())
    }

    pub fn emit_message(&self, msg: &Act) -> Result<()> {
        debug!("emit_message: {:?}", msg);
        let workflow = self.proc.model();
        let mut inputs = utils::fill_inputs(&msg.inputs, self);

        let task = self.task();
        if let Some(err) = task.err() {
            inputs.set(consts::ACT_ERR_MESSAGE, err.message);
            inputs.set(consts::ACT_ERR_CODE, err.ecode);
        }

        let state: MessageState = task.state().into();
        let msg = Message {
            id: utils::longid(),
            r#type: consts::ACT_TYPE_MSG.to_string(),
            source: task.node().kind().to_string(),
            state,
            pid: task.pid.clone(),
            tid: task.id.clone(),
            key: msg.key.to_string(),
            name: task.node().name(),

            model: Model {
                id: workflow.id.clone(),
                name: workflow.name.to_string(),
                tag: workflow.tag.to_string(),
            },

            tag: msg.tag.to_string(),
            inputs,
            ..Default::default()
        };

        self.runtime.emitter().emit_message(&msg);
        Ok(())
    }
}
