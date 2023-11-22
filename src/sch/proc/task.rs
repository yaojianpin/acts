mod act;
mod branch;
// mod job;
mod step;
mod workflow;

use crate::{
    env::Room,
    event::{ActionState, EventAction},
    sch::{
        tree::{Node, NodeData},
        Context, Proc, Scheduler, TaskState,
    },
    utils::{self, consts},
    ActError, ActTask, Error, Message, NodeKind, Result, ShareLock, Vars, WorkflowAction,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, RwLock};
use tracing::info;

#[derive(Clone)]
pub struct Task {
    /// proc id
    pub proc_id: String,

    /// task id
    pub id: String,

    /// task node
    pub node: Arc<Node>,

    pub timestamp: i64,

    /// env room
    room: Arc<Room>,

    /// task state
    state: ShareLock<TaskState>,

    /// action state by do_action function
    action_state: ShareLock<ActionState>,

    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    // previous tid
    prev: ShareLock<Option<String>>,

    proc: Arc<Proc>,
}

impl Task {
    pub fn new(proc: &Arc<Proc>, task_id: &str, node: Arc<Node>) -> Self {
        // create new env for each task
        let room = proc.env().new_room();
        let task = Self {
            proc_id: proc.id(),
            id: task_id.to_string(),
            node: node.clone(),
            state: Arc::new(RwLock::new(TaskState::None)),
            action_state: Arc::new(RwLock::new(ActionState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            prev: Arc::new(RwLock::new(None)),
            room,
            timestamp: utils::time::timestamp(),
            proc: proc.clone(),
        };

        task
    }

    pub fn proc(&self) -> &Arc<Proc> {
        &self.proc
    }

    pub fn room(&self) -> &Arc<Room> {
        &self.room
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
    pub fn node_id(&self) -> String {
        self.node.id()
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
    }

    pub fn is_emit_disabled(&self) -> bool {
        self.room
            .get(consts::TASK_EMIT_DISABLED)
            .map(|v| v.as_bool().unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn set_emit_disabled(&self, v: bool) {
        self.room.set(consts::TASK_EMIT_DISABLED, json!(v));
    }

    pub fn parent_task_id(&self) -> Option<String> {
        self.room
            .get(consts::PARENT_TASK_ID)
            .map(|v| v.as_str().map(|v| v.to_string()).unwrap())
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
                        consts::FOR_ACT_KEY_STEP_NODE_ID.to_string(),
                        json!(task.node_id()),
                    );
                    inputs.insert(consts::FOR_ACT_KEY_STEP_TASK_ID.to_string(), json!(task.id));
                    break;
                }
                parent = task.parent();
            }
        }
        Message {
            id: self.id.clone(),
            name: self.node.data().name(),
            r#type: self.node.kind().to_string(),
            state: self.action_state().to_string(),
            proc_id: self.proc_id.clone(),
            key: self.node_id(),
            tag: self.node.tag(),

            model_id: workflow.id.clone(),
            model_name: workflow.name.to_string(),
            model_tag: workflow.tag.to_string(),

            inputs,
            outputs: self.node.outputs(),

            start_time: self.start_time(),
            end_time: self.end_time(),
        }
    }

    pub fn create_action_message(&self, action: &WorkflowAction) -> Message {
        let workflow = self.proc.model();
        let inputs = utils::fill_inputs(&self.room, &action.inputs);
        Message {
            id: utils::shortid(),
            r#type: "message".to_string(),
            state: self.action_state().to_string(),
            proc_id: self.proc_id.clone(),
            key: action.id.clone(),
            name: action.name.clone(),

            model_id: workflow.id.clone(),
            model_name: workflow.name.to_string(),
            model_tag: workflow.tag.to_string(),

            inputs,
            outputs: action.outputs.clone(),

            start_time: self.start_time(),
            end_time: self.end_time(),
            ..Default::default()
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
        utils::fill_inputs(&self.room, &self.node.inputs())
    }

    pub fn outputs(&self) -> Vars {
        utils::fill_outputs(&self.room, &self.node.outputs())
    }

    pub fn vars(&self) -> Vars {
        self.room.vars()
    }

    pub fn set_prev(&self, prev: Option<String>) {
        *self.prev.write().unwrap() = prev;
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

    pub fn exec(&self, ctx: &Context) -> Result<()> {
        info!("exec task={:?}", ctx.task);
        self.init(ctx)?;
        self.run(ctx)?;
        self.next(ctx)?;
        Ok(())
    }

    pub fn update(&self, ctx: &Context) -> Result<()> {
        info!("update task={:?}", ctx.task);
        let action = ctx
            .action()
            .ok_or(ActError::Action(format!("cannot find action in context")))?;
        // check uid
        // ctx.var(consts::FOR_ACT_KEY_UID)
        //     .map(|v| v.as_str().unwrap().to_string())
        //     .ok_or(ActError::Action(format!(
        //         "cannot find '{}' in options",
        //         consts::FOR_ACT_KEY_UID
        //     )))?;

        match action {
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
                let to = ctx.var("to").ok_or(ActError::Action(format!(
                    "cannot find to value in options",
                )))?;
                let nid = to.as_str().ok_or(ActError::Action(format!(
                    "to '{to}' value is not a valid string type",
                )))?;

                let mut path_tasks = Vec::new();
                let task = self.backs(
                    &|t| t.node.kind() == NodeKind::Step && t.node_id() == nid,
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
                    ctx.var(consts::FOR_ACT_KEY_STEP_ERR_CODE)
                        .ok_or(ActError::Action(format!(
                            "cannot find '{}' in options",
                            consts::FOR_ACT_KEY_STEP_ERR_CODE
                        )))?;
                let err_code = err_code.as_str().unwrap_or_default();
                if err_code.is_empty() {
                    return Err(ActError::Action(format!(
                        "the var '{}' cannot be empty",
                        consts::FOR_ACT_KEY_STEP_ERR_CODE
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
                    .var(consts::FOR_ACT_KEY_STEP_ERR_MESSAGE)
                    .map(|e| e.as_str().unwrap().to_string())
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
        match self.node.data() {
            NodeData::Branch(n) => {
                let siblings = self.siblings();
                if n.needs.len() > 0 {
                    if siblings
                        .iter()
                        .filter(|iter| {
                            iter.state().is_completed() && n.needs.contains(&iter.node_id())
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
            NodeData::Act(n) => {
                if n.needs.len() > 0 {
                    let siblings = self.siblings();
                    if siblings
                        .iter()
                        .filter(|iter| {
                            iter.state().is_completed() && n.needs.contains(&iter.node_id())
                        })
                        .count()
                        > 0
                    {
                        return true;
                    }

                    return false;
                }

                true
            }
            _ => true,
        }
    }

    pub fn resume(&self, ctx: &Context) -> Result<()> {
        if self.is_ready() {
            self.set_state(TaskState::Running);
            ctx.scher.emitter().emit_task_event(self);
            self.exec(&ctx)?;
        }

        Ok(())
    }
    /// check if the task includes branches
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
impl ActTask for Task {
    fn init(&self, ctx: &Context) -> Result<()> {
        if ctx.task.state().is_none() {
            ctx.prepare();
            self.set_action_state(ActionState::Created);
            match &self.node.data {
                NodeData::Workflow(workflow) => workflow.init(ctx)?,
                NodeData::Branch(branch) => branch.init(ctx)?,
                NodeData::Step(step) => step.init(ctx)?,
                NodeData::Act(act) => act.init(ctx)?,
            }

            ctx.emit_task(&ctx.task);
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if ctx.task.state().is_running() {
            match &self.node.data {
                NodeData::Workflow(workflow) => workflow.run(ctx),
                NodeData::Branch(branch) => branch.run(ctx),
                NodeData::Step(step) => step.run(ctx),
                NodeData::Act(act) => act.run(ctx),
            }?;
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let mut is_next = false;
        if ctx.task.state().is_next() {
            is_next = match &self.node.data {
                NodeData::Workflow(data) => data.next(ctx)?,
                NodeData::Step(data) => data.next(ctx)?,
                NodeData::Branch(data) => data.next(ctx)?,
                NodeData::Act(data) => data.next(ctx)?,
            };
        }

        info!("is_next:{} task={:?}", is_next, ctx.task);
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
        let is_review = match &self.node.data {
            NodeData::Workflow(data) => data.review(ctx)?,
            NodeData::Step(data) => data.review(ctx)?,
            NodeData::Branch(data) => data.review(ctx)?,
            NodeData::Act(data) => data.review(ctx)?,
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
        match &self.node.data {
            NodeData::Workflow(data) => data.error(ctx),
            NodeData::Step(data) => data.error(ctx),
            NodeData::Branch(data) => data.error(ctx),
            NodeData::Act(data) => data.error(ctx),
        }
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("name", &self.node.name())
            .field("kind", &self.node.kind())
            .field("proc_id", &self.proc_id)
            .field("node_id", &self.node.id())
            .field("state", &self.state())
            .field("action_state", &self.action_state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("prev", &self.prev())
            .field("vars", &self.vars())
            .finish()
    }
}
