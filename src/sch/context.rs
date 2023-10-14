use crate::{
    event::{Action, ActionState, EventAction},
    model,
    sch::{tree::NodeData, Node, Proc, Scheduler, Task},
    ActError, ActValue, NodeKind, Result, ShareLock, TaskState, Vars, WorkflowAction,
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
            .field("pid", &self.proc.id())
            .field("tid", &self.task.id)
            .field("action", &self.action())
            .finish()
    }
}

impl Context {
    fn init_vars(&self, task: &Task) {
        // let vars = task.node.inputs();
        let inputs = task.inputs();
        self.task.room().append(&inputs);
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
        self.task.room().bind_context(self);
        self.init_vars(&self.task);
    }

    pub fn set_action(&self, action: &Action) -> Result<()> {
        *self.action.write().unwrap() = Some(EventAction::parse(action.event.as_str())?);
        self.task.room().append(&action.options);

        Ok(())
    }

    pub fn run(&self, script: &str) -> Result<bool> {
        self.task.room().run(script)
    }

    pub fn eval(&self, expr: &str) -> Result<bool> {
        self.task.room().eval(expr)
    }

    pub fn eval_with<T: rhai::Variant + Clone>(&self, expr: &str) -> Result<T> {
        self.task.room().eval(expr)
    }

    pub fn var(&self, name: &str) -> Option<ActValue> {
        self.task.room().get(name)
    }

    pub fn set_var(&self, name: &str, value: ActValue) {
        self.task.room().set(name, value)
    }

    #[allow(unused)]
    pub(in crate::sch) fn action(&self) -> Option<EventAction> {
        self.action.read().unwrap().clone()
    }

    pub fn sched_task(&self, node: &Arc<Node>) {
        let task = self.proc.create_task(&node, Some(self.task.clone()));
        self.scher.push(&task);
    }

    pub fn sched_act(&self, id: &str, tag: &str, inputs: &Vars, outputs: &Vars) {
        let act = model::Act {
            id: id.to_string(),
            tag: tag.to_string(),
            inputs: inputs.clone(),
            outputs: outputs.clone(),
            ..Default::default()
        };

        let node = self
            .proc
            .tree()
            .make(NodeData::Act(act), self.task.node.level + 1);
        node.set_parent(&self.task.node);

        if self.task.state().is_pending() {
            let task = self.proc.create_task(&node, Some(self.task.clone()));
            self.scher.push(&task);
        }
    }

    /// redo the task and dispatch directly
    pub fn redo_task(&self, task: &Arc<Task>) -> Result<()> {
        if let Some(prev) = task.prev() {
            if let Some(prev_task) = self.task.proc().task(&prev) {
                let task = self.proc.create_task(&task.node, Some(prev_task));

                let vars = task.room().vars();
                task.room().append(&vars);
                self.scher.push(&task);
            }
        }

        Ok(())
    }

    pub fn back_task(&self, task: &Arc<Task>) -> Result<()> {
        let parent = task.parent().ok_or(ActError::Action(format!(
            "cannot find task parent by tid '{}'",
            task.id
        )))?;

        let tasks = parent.children();
        for task in tasks
            .iter()
            .filter(|t| t.id != task.id && !t.state().is_completed())
        {
            task.set_action_state(ActionState::Skipped);
            self.emit_task(task);
        }

        task.set_action_state(ActionState::Backed);
        self.emit_task(task);

        parent.set_action_state(ActionState::Backed);
        self.emit_task(&parent);

        Ok(())
    }

    pub fn abort_task(&self, task: &Arc<Task>) -> Result<()> {
        let parent = task.parent().ok_or(ActError::Action(format!(
            "cannot find task parent by tid '{}'",
            task.id
        )))?;

        let tasks = parent.children();

        // abort all task's act
        for task in tasks
            .iter()
            .filter(|t| t.id != task.id && !t.state().is_completed())
        {
            task.set_action_state(ActionState::Skipped);
            self.emit_task(task);
        }

        task.set_action_state(ActionState::Aborted);
        self.emit_task(task);

        // abort all running task
        let ctx = self;
        let mut parent = task.parent();
        while let Some(task) = parent {
            let proc = ctx.proc.clone();
            let ctx = proc.create_context(&ctx.scher, &task);
            ctx.task.set_action_state(ActionState::Aborted);
            ctx.emit_task(&ctx.task);

            for t in task.children() {
                if t.state().is_pending() || t.state().is_running() {
                    t.set_action_state(ActionState::Aborted);
                    ctx.emit_task(&t);
                }
            }

            parent = task.parent();
        }
        Ok(())
    }

    /// undo task
    pub fn undo_task(&self, task: &Arc<Task>) -> Result<()> {
        if task.state().is_completed() {
            return Err(ActError::Action(format!(
                "task('{}') is not allowed to cancel",
                task.id
            )));
        }

        // skip all of the task's sub tasks
        for sub in task
            .children()
            .iter()
            .filter(|t| t.id != task.id && !t.state().is_completed())
        {
            sub.set_action_state(ActionState::Cancelled);
            self.emit_task(&sub);
        }
        task.set_action_state(ActionState::Cancelled);
        self.emit_task(&task);

        Ok(())
    }

    pub fn skip_task(&self, task: &Arc<Task>) -> Result<()> {
        if task.state().is_completed() {
            return Err(ActError::Action(format!(
                "task '{}' is already completed",
                task.id
            )));
        }
        let parent = task.parent().ok_or(ActError::Action(format!(
            "cannot find task parent by tid '{}'",
            task.id
        )))?;

        // skip all of the task's sub tasks
        for sub in parent
            .children()
            .iter()
            .filter(|t| t.id != task.id && !t.state().is_completed())
        {
            sub.set_action_state(ActionState::Skipped);
            self.emit_task(&sub);
        }
        task.set_action_state(ActionState::Skipped);
        self.emit_task(&task);

        Ok(())
    }
    pub fn emit_error(&self) {
        let state = self.task.state();
        if state.is_error() {
            let mut parent = self.task.parent();
            while let Some(task) = &parent {
                task.set_state(state.clone());
                //self.scher.emitter().dispatch_task_event(task, &data);

                parent = task.parent();
            }

            // dispatch workflow event
            self.proc.set_state(state.clone());
            self.scher.emitter().emit_proc_event(&self.proc);
        }
    }

    pub fn emit_task(&self, task: &Task) {
        debug!("ctx::emit_task, task={:?}", task);
        // on workflow start
        if let NodeData::Workflow(_) = &task.node.data {
            if task.action_state() == ActionState::Created {
                self.proc.set_state(TaskState::Running);
                self.scher.emitter().emit_proc_event(&self.proc);
            }
        }

        if task.action_state() == ActionState::Completed {
            let outputs = task.outputs();
            match task.node.kind() {
                NodeKind::Act => {
                    // only add outputs to its parent task for act node
                    if let Some(parent) = task.parent() {
                        parent.room().append(&outputs);
                    }
                }
                _ => {
                    // add outputs to proc global for other node
                    task.proc().env().append(&outputs);
                }
            }
        }

        self.scher.emitter().emit_task_event(task);

        // on workflow complete
        if let NodeData::Workflow(_) = &task.node.data {
            if task.action_state() != ActionState::Created {
                self.proc.set_state(task.state());
                self.scher.emitter().emit_proc_event(&self.proc);
            }
        }
    }

    pub fn send_message(&self, key: &str) {
        let message = self.task.create_action_message(&WorkflowAction {
            name: "message".to_string(),
            id: key.to_string(),
            ..Default::default()
        });
        self.scher.emitter().emit_message(&message);
    }
}
