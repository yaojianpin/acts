mod act;
mod branch;
mod hook;
mod step;
mod workflow;

use crate::{
    env::RefEnv,
    event::{ActionState, EventAction, Model},
    sch::{
        tree::{Node, NodeContent},
        Context, Proc, Scheduler, TaskState,
    },
    utils::{self, consts},
    Act, ActError, ActTask, Catch, Error, Message, NodeKind, Req, Result, ShareLock, Timeout, Vars,
};
use async_trait::async_trait;
pub use hook::{StatementBatch, TaskLifeCycle};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tracing::{debug, info};

#[derive(Clone)]
pub struct Task {
    /// proc id
    pub proc_id: String,

    /// task id
    pub id: String,

    /// task node
    pub node: Arc<Node>,

    pub timestamp: i64,

    // ref of the Enviroment
    env: Arc<RefEnv>,

    /// task state
    state: ShareLock<TaskState>,

    /// action state by do_action function
    action_state: ShareLock<ActionState>,

    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    // previous tid
    prev: ShareLock<Option<String>>,

    proc: Arc<Proc>,

    // lifecycle hooks
    hooks: ShareLock<HashMap<TaskLifeCycle, Vec<StatementBatch>>>,
}

impl Task {
    pub fn new(proc: &Arc<Proc>, task_id: &str, node: Arc<Node>) -> Self {
        let env = proc.env().create_ref(task_id);
        let task = Self {
            proc_id: proc.id(),
            id: task_id.to_string(),
            node: node.clone(),
            env,
            state: Arc::new(RwLock::new(TaskState::None)),
            action_state: Arc::new(RwLock::new(ActionState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            prev: Arc::new(RwLock::new(None)),
            timestamp: utils::time::timestamp(),
            proc: proc.clone(),
            hooks: Arc::new(RwLock::new(HashMap::new())),
        };

        task
    }

    pub fn proc(&self) -> &Arc<Proc> {
        &self.proc
    }

    pub fn env(&self) -> &Arc<RefEnv> {
        &self.env
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read().unwrap()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read().unwrap()
    }

    pub fn state(&self) -> TaskState {
        let state = &*self.state.read().unwrap();
        state.clone()
    }

    pub fn action_state(&self) -> ActionState {
        let state = &*self.action_state.read().unwrap();
        state.clone()
    }

    pub fn task_id(&self) -> String {
        self.id.clone()
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
    }

    pub fn is_emit_disabled(&self) -> bool {
        self.env()
            .get::<bool>(consts::TASK_EMIT_DISABLED)
            .unwrap_or(false)
    }

    pub fn set_emit_disabled(&self, v: bool) {
        self.env().set(consts::TASK_EMIT_DISABLED, json!(v));
    }

    pub fn create_context(self: &Arc<Self>, scher: &Arc<Scheduler>) -> Arc<Context> {
        self.proc.create_context(scher, self)
    }

    pub fn create_message(&self) -> Message {
        let workflow = self.proc.model();

        // if it is act, insert the step_node_id and step_task_id to the inputs
        // it is necessary to find the relation between the step and it's children acts
        let mut inputs = self.inputs();

        if self.node.kind() == NodeKind::Act {
            let mut parent = self.parent();
            while let Some(task) = parent {
                if task.is_kind(NodeKind::Step) {
                    inputs.insert(
                        consts::STEP_KEY.to_string(),
                        json!({
                            consts::STEP_NODE_ID: task.node.id(),
                            consts::STEP_NODE_NAME: task.node.name(),
                            consts::STEP_TASK_ID: task.id,
                        }),
                    );
                    break;
                }
                parent = task.parent();
            }
        }

        // if there is no key, use id instead
        let mut key = self.node.key();
        if key.is_empty() {
            key = self.node.id();
        }

        Message {
            id: self.id.clone(),
            name: self.node.content.name(),
            r#type: self.node.r#type(),
            source: self.node.kind().to_string(),
            state: self.action_state().to_string(),
            proc_id: self.proc_id.clone(),
            key: key.to_string(),
            tag: self.node.tag().to_string(),

            model: Model {
                id: workflow.id.clone(),
                name: workflow.name.clone(),
                tag: workflow.tag.clone(),
            },

            inputs,
            outputs: self.outputs(),

            start_time: self.start_time(),
            end_time: self.end_time(),
        }
    }

    pub fn prev(&self) -> Option<String> {
        let ret = self.prev.read().unwrap();
        ret.clone()
    }

    pub fn parent(&self) -> Option<Arc<Task>> {
        let mut prev = self.prev();
        while let Some(tid) = prev.clone() {
            match self.proc.task(&tid) {
                Some(task) => {
                    if task.node.level < self.node.level {
                        return Some(task.clone());
                    }

                    prev = task.prev();
                    continue;
                }
                None => {
                    break;
                }
            }
        }

        None
    }

    pub fn children(&self) -> Vec<Arc<Self>> {
        self.proc.children(&self.id)
    }

    pub fn siblings(&self) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        if let Some(parent) = self.parent() {
            let children = parent.children();
            ret.extend(children.iter().filter(|iter| iter.id != self.id).cloned());
        }

        ret
    }

    pub fn inputs(&self) -> Vars {
        utils::fill_inputs(self.env(), &self.node.content.inputs())
    }

    pub fn outputs(&self) -> Vars {
        let mut outputs = utils::fill_outputs(self.env(), &self.node.content.outputs());

        // The task(Workflow) can also set the outputs by expose act
        // These data is stored in consts::ACT_OUTPUTS
        // merge the expose data with the outputs
        if let Some(vars) = self.env().get::<Vars>(consts::ACT_OUTPUTS) {
            for (key, value) in &vars {
                outputs.set(key, value.clone());
            }
        }

        outputs
    }

    pub fn data(&self) -> Vars {
        self.env().data()
    }

    pub fn set_prev(&self, prev: Option<String>) {
        {
            *self.prev.write().unwrap() = prev;
        }

        // set refenv parent task id
        if let Some(parent) = self.parent() {
            self.env().set_parent(&parent.id);
        }
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            self.set_end_time(utils::time::time());
        } else if state.is_running() || state.is_interrupted() {
            self.set_start_time(utils::time::time());
        }
        *self.state.write().unwrap() = state;
    }

    pub fn set_action_state(&self, state: ActionState) {
        match state {
            ActionState::None => self.set_state(TaskState::None),
            ActionState::Created => {
                if self.state().is_none() {
                    self.set_state(TaskState::Running);
                }
            }
            ActionState::Cancelled
            | ActionState::Backed
            | ActionState::Submitted
            | ActionState::Completed => self.set_state(TaskState::Success),
            ActionState::Aborted => self.set_state(TaskState::Abort),
            ActionState::Skipped => self.set_state(TaskState::Skip),
            ActionState::Error => self.set_state(TaskState::Fail(format!("action error"))),
            ActionState::Removed => self.set_state(TaskState::Removed),
        }

        *self.action_state.write().unwrap() = state;
    }

    pub fn set_pure_state(&self, state: TaskState) {
        *self.state.write().unwrap() = state;
    }

    pub fn set_pure_action_state(&self, state: ActionState) {
        *self.action_state.write().unwrap() = state;
    }

    pub fn set_start_time(&self, time: i64) {
        *self.start_time.write().unwrap() = time;
    }
    pub fn set_end_time(&self, time: i64) {
        *self.end_time.write().unwrap() = time;
    }

    pub fn is_kind(&self, kind: NodeKind) -> bool {
        self.node.kind() == kind
    }

    pub fn is_act(&self, v: &str) -> bool {
        if self.node.kind() == NodeKind::Act {
            return self.node.r#type() == v;
        }
        false
    }

    pub fn exec(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        info!("exec task={:?}", ctx.task);
        self.init(ctx)?;
        self.run(ctx)?;
        self.next(ctx)?;
        Ok(())
    }

    pub fn update(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        info!("update task={:?}", ctx.task);
        let action = ctx
            .action()
            .ok_or(ActError::Action(format!("cannot find action in context")))?;
        match action {
            EventAction::Push => {
                let act = Act::Req(Req {
                    id: ctx
                        .get_var::<String>("id")
                        .ok_or(ActError::Runtime(format!("cannot find 'id' in options")))?,
                    name: ctx.get_var::<String>("name").unwrap_or_default(),
                    tag: ctx.get_var::<String>("tag").unwrap_or_default(),
                    key: ctx.get_var::<String>("key").unwrap_or_default(),
                    inputs: ctx.get_var("inputs").unwrap_or_default(),
                    outputs: ctx.get_var("outputs").unwrap_or_default(),
                    rets: ctx.get_var("rets").unwrap_or_default(),
                    ..Default::default()
                });
                ctx.append_act(&act)?;
            }
            EventAction::Remove => {
                self.set_action_state(ActionState::Removed);
                self.next(ctx)?;
            }
            EventAction::Submit => {
                self.set_action_state(ActionState::Submitted);
                self.next(ctx)?;
            }
            EventAction::Complete => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}' is already completed",
                        self.id
                    )));
                }
                self.set_action_state(ActionState::Completed);
                self.next(ctx)?;
            }
            EventAction::Back => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}' is already completed",
                        self.id
                    )));
                }
                let nid = ctx.get_var::<String>("to").ok_or(ActError::Action(format!(
                    "cannot find 'to' value in options",
                )))?;

                let mut path_tasks = Vec::new();
                let task = self.backs(
                    &|t| t.node.kind() == NodeKind::Step && t.node.id() == nid,
                    &mut path_tasks,
                );

                let task = task.ok_or(ActError::Action(format!(
                    "cannot find history task by nid '{}'",
                    nid
                )))?;

                ctx.back_task(&ctx.task, &path_tasks)?;
                ctx.redo_task(&task)?;
            }
            EventAction::Cancel => {
                // find the parent step task
                let mut step = ctx.task.parent();
                while let Some(task) = &step {
                    if task.is_kind(NodeKind::Step) {
                        break;
                    }
                    step = task.parent();
                }

                let task = step.ok_or(ActError::Action(format!(
                    "cannot find parent step task by tid '{}'",
                    ctx.task.id,
                )))?;
                if !task.state().is_success() {
                    return Err(ActError::Action(format!(
                        "task('{}') is not allowed to cancel",
                        task.id
                    )));
                }
                // get the neartest next step tasks
                let mut path_tasks = Vec::new();
                let nexts = task.follows(
                    &|t| t.is_kind(NodeKind::Step) && t.is_acts(),
                    &mut path_tasks,
                );
                if nexts.len() == 0 {
                    return Err(ActError::Action(format!("cannot find cancelled tasks")));
                }

                // mark the path tasks as completed
                for p in path_tasks {
                    if p.state().is_running() {
                        p.set_action_state(ActionState::Completed);
                        ctx.emit_task(&p);
                    } else if p.state().is_pending() {
                        p.set_action_state(ActionState::Skipped);
                        ctx.emit_task(&p);
                    }
                }

                for next in &nexts {
                    ctx.undo_task(next)?;
                }
                ctx.redo_task(&task)?;
            }
            EventAction::Abort => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}' is already completed",
                        self.id
                    )));
                }
                ctx.abort_task(&ctx.task)?;
            }
            EventAction::Skip => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}' is already completed",
                        self.id
                    )));
                }

                for task in self.siblings() {
                    if task.state().is_completed() {
                        continue;
                    }
                    task.set_action_state(ActionState::Skipped);
                    ctx.emit_task(&task);
                }

                // set both current act and parent step to skip
                self.set_action_state(ActionState::Skipped);
                self.next(ctx)?;
            }
            EventAction::Error => {
                let err_code =
                    ctx.get_var::<String>(consts::ACT_ERR_CODE)
                        .ok_or(ActError::Action(format!(
                            "cannot find '{}' in options",
                            consts::ACT_ERR_CODE
                        )))?;
                if err_code.is_empty() {
                    return Err(ActError::Action(format!(
                        "the var '{}' cannot be empty",
                        consts::ACT_ERR_CODE
                    )));
                }

                let task = &ctx.task;
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

                for sub in parent.siblings().iter() {
                    if sub.state().is_completed() {
                        continue;
                    }
                    sub.set_action_state(ActionState::Skipped);
                    ctx.emit_task(&sub);
                }

                let err_message = ctx
                    .get_var::<String>(consts::ACT_ERR_MESSAGE)
                    .unwrap_or_default();
                ctx.set_err(&Error {
                    key: Some(err_code.to_string()),
                    message: err_message,
                });
                task.error(ctx)?;
            }
        }

        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        match &self.node.content {
            NodeContent::Branch(n) => {
                let siblings = self.siblings();
                if n.needs.len() > 0 {
                    if siblings
                        .iter()
                        .filter(|iter| {
                            iter.state().is_completed()
                                && n.needs.contains(&iter.node.id().to_string())
                        })
                        .count()
                        > 0
                    {
                        return true;
                    }
                    return false;
                }

                if n.r#else {
                    if siblings.iter().all(|iter| iter.state().is_skip()) {
                        return true;
                    }

                    // fix the branch.default state
                    if siblings.iter().any(|iter| {
                        iter.state().is_error()
                            || iter.state().is_success()
                            || iter.state().is_abort()
                    }) {
                        self.set_action_state(ActionState::Skipped);
                    }
                }

                false
            }
            // NodeData::Act(n) => {
            //     if n.needs.len() > 0 {
            //         let siblings = self.siblings();
            //         if siblings
            //             .iter()
            //             .filter(|iter| {
            //                 iter.state().is_completed() && n.needs.contains(&iter.node_id())
            //             })
            //             .count()
            //             > 0
            //         {
            //             return true;
            //         }

            //         return false;
            //     }

            //     true
            // }
            _ => true,
        }
    }

    pub fn resume(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        if self.is_ready() {
            self.set_state(TaskState::Running);
            ctx.scher.emitter().emit_task_event(self);
            self.exec(&ctx)?;
        }

        Ok(())
    }

    /// add statement to task lifecycle hooks
    pub fn add_hook_stmts(&self, key: TaskLifeCycle, value: &Act) {
        let mut hooks = self.hooks.write().unwrap();

        let batch = StatementBatch::Statement(value.clone());
        hooks
            .entry(key)
            .and_modify(|list| list.push(batch.clone()))
            .or_insert(vec![batch]);
    }

    pub fn add_hook_catch(&self, key: TaskLifeCycle, value: &Catch) {
        let mut hooks = self.hooks.write().unwrap();

        let batch = StatementBatch::Catch(value.clone());
        hooks
            .entry(key)
            .and_modify(|list| list.push(batch.clone()))
            .or_insert(vec![batch]);
    }

    pub fn add_hook_timeout(&self, key: TaskLifeCycle, value: &Timeout) {
        let mut hooks = self.hooks.write().unwrap();

        let batch = StatementBatch::Timeout(value.clone());
        hooks
            .entry(key)
            .and_modify(|list| list.push(batch.clone()))
            .or_insert(vec![batch]);
    }

    pub fn run_hooks(&self, ctx: &Context) -> Result<()> {
        let state = self.action_state();
        match state {
            ActionState::None => {}
            ActionState::Created => {
                self.run_hooks_by(TaskLifeCycle::Created, ctx)?;
                if self.is_kind(NodeKind::Act) {
                    if let Some(task) = self.parent() {
                        task.run_hooks_by(TaskLifeCycle::BeforeUpdate, ctx)?;
                    }
                    if let Some(root) = ctx.task.proc.root() {
                        root.run_hooks_by(TaskLifeCycle::BeforeUpdate, ctx)?
                    }
                } else if self.is_kind(NodeKind::Step) {
                    ctx.task.run_hooks_by(TaskLifeCycle::Step, ctx)?;
                    if let Some(root) = ctx.task.proc.root() {
                        root.run_hooks_by(TaskLifeCycle::Step, ctx)?
                    }
                }
            }
            ActionState::Completed
            | ActionState::Submitted
            | ActionState::Backed
            | ActionState::Cancelled
            | ActionState::Aborted
            | ActionState::Skipped
            | ActionState::Removed => {
                self.run_hooks_by(TaskLifeCycle::Completed, ctx)?;
                if self.is_kind(NodeKind::Act) {
                    // triggers step updated hook when the act is completed
                    if let Some(task) = self.parent() {
                        task.run_hooks_by(TaskLifeCycle::Updated, ctx)?;
                    }
                    if let Some(root) = ctx.task.proc.root() {
                        root.run_hooks_by(TaskLifeCycle::Updated, ctx)?
                    }
                } else if self.is_kind(NodeKind::Step) {
                    ctx.task.run_hooks_by(TaskLifeCycle::Step, ctx)?;
                    if let Some(root) = ctx.task.proc.root() {
                        root.run_hooks_by(TaskLifeCycle::Step, ctx)?
                    }
                }
            }
            ActionState::Error => self.run_hooks_by(TaskLifeCycle::ErrorCatch, ctx)?,
        }

        Ok(())
    }

    pub fn run_hooks_timeout(&self, ctx: &Context) -> Result<()> {
        self.run_hooks_by(TaskLifeCycle::Timeout, ctx)
    }

    fn run_hooks_by(&self, key: TaskLifeCycle, ctx: &Context) -> Result<()> {
        debug!("run_hooks_by:{:?} {:?}", key, self);
        let hooks = self.hooks.read().unwrap();
        let default = Vec::new();
        let stmts = hooks.get(&key).unwrap_or(&default);
        for s in stmts {
            s.run(ctx)?;
        }
        Ok(())
    }

    pub(crate) fn set_hooks(&self, hooks: &HashMap<TaskLifeCycle, Vec<StatementBatch>>) {
        *self.hooks.write().unwrap() = hooks.clone();
    }

    pub(crate) fn hooks(&self) -> HashMap<TaskLifeCycle, Vec<StatementBatch>> {
        self.hooks.read().unwrap().clone()
    }

    /// check if the task includes act
    fn is_acts(&self) -> bool {
        self.children()
            .iter()
            .any(|iter| iter.is_kind(NodeKind::Act))
    }

    fn backs<F: Fn(&Arc<Self>) -> bool + Clone>(
        &self,
        predicate: &F,
        path: &mut Vec<Arc<Self>>,
    ) -> Option<Arc<Self>> {
        let mut ret = None;

        let mut prev = self.prev();
        while let Some(tid) = &prev {
            if let Some(task) = self.proc.task(tid) {
                if predicate(&task) {
                    ret = Some(task.clone());
                    break;
                }

                // push the path tasks
                if task.state().is_running() || task.state().is_pending() {
                    path.push(task.clone());
                }

                prev = task.prev();
            } else {
                prev = None
            }
        }

        ret
    }

    fn follows<F: Fn(&Arc<Self>) -> bool + Clone>(
        &self,
        predicate: &F,
        path: &mut Vec<Arc<Self>>,
    ) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        let children = self.children();
        if children.len() > 0 {
            for task in &children {
                if predicate(task) {
                    ret.push(task.clone());
                } else {
                    // push the path tasks
                    if task.state().is_running() || task.state().is_pending() {
                        path.push(task.clone());
                    }

                    // find the next follows
                    ret.extend(task.follows(predicate, path).into_iter());
                }
            }
        }

        ret
    }
}

#[async_trait]
impl ActTask for Arc<Task> {
    fn init(&self, ctx: &Context) -> Result<()> {
        if ctx.task.state().is_none() {
            ctx.prepare();
            match &self.node.content {
                NodeContent::Workflow(workflow) => workflow.init(ctx)?,
                NodeContent::Branch(branch) => branch.init(ctx)?,
                NodeContent::Step(step) => step.init(ctx)?,
                NodeContent::Act(act) => act.init(ctx)?,
            }
            self.set_action_state(ActionState::Created);
            ctx.emit_task(&ctx.task);
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if ctx.task.state().is_running() {
            match &self.node.content {
                NodeContent::Workflow(workflow) => workflow.run(ctx),
                NodeContent::Branch(branch) => branch.run(ctx),
                NodeContent::Step(step) => step.run(ctx),
                NodeContent::Act(act) => act.run(ctx),
            }?;
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let mut is_next = false;
        if ctx.task.state().is_next() {
            is_next = match &self.node.content {
                NodeContent::Workflow(data) => data.next(ctx)?,
                NodeContent::Step(data) => data.next(ctx)?,
                NodeContent::Branch(data) => data.next(ctx)?,
                NodeContent::Act(data) => data.next(ctx)?,
            };
        }

        debug!("is_next:{} task={:?}", is_next, ctx.task);
        self.env().set_env(&ctx.vars());
        if self.state().is_completed() {
            ctx.emit_task(self);
        }

        if !is_next {
            let parent = ctx.task.parent();
            if let Some(task) = &parent.clone() {
                let ctx = task.create_context(&ctx.scher);
                task.review(&ctx)?;
            }
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let is_review = match &self.node.content {
            NodeContent::Workflow(data) => data.review(ctx)?,
            NodeContent::Step(data) => data.review(ctx)?,
            NodeContent::Branch(data) => data.review(ctx)?,
            NodeContent::Act(data) => data.review(ctx)?,
        };

        info!("is_review:{} task={:?}", is_review, ctx.task);
        if self.state().is_completed() {
            ctx.emit_task(self);
        }

        if is_review {
            let parent = ctx.task.parent();
            if let Some(task) = &parent.clone() {
                let ctx = task.create_context(&ctx.scher);
                return task.review(&ctx);
            }
        }

        Ok(false)
    }

    fn error(&self, ctx: &Context) -> Result<()> {
        match &self.node.content {
            NodeContent::Workflow(data) => data.error(ctx),
            NodeContent::Step(data) => data.error(ctx),
            NodeContent::Branch(data) => data.error(ctx),
            NodeContent::Act(data) => data.error(ctx),
        }
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("name", &self.node.name())
            .field("type", &self.node.kind())
            .field("proc_id", &self.proc_id)
            .field("node_id", &self.node.id())
            .field("state", &self.state())
            .field("action_state", &self.action_state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("prev", &self.prev())
            .field("vars", &self.data())
            .finish()
    }
}
