mod act;
mod branch;
mod step;
mod workflow;

use crate::ActRunAs;
use crate::scheduler::{NextAction, Sign};
use crate::store::DbCollectionIden;
use crate::utils::consts::TASK_ROOT_TID;
use crate::{
    Act, ActError, ActTask, Error, Message, MessageState, NodeKind, Result, ShareLock, Variant,
    Vars,
    data::{self, MessageStatus},
    event::EventAction,
    scheduler::{
        Context, Process, Runtime, TaskState,
        tree::{Node, NodeContent},
    },
    utils::{self, consts},
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::{Arc, RwLock};
use tracing::debug;

#[derive(Clone)]
pub struct Task {
    /// process id
    pub pid: String,

    /// task id
    pub id: String,

    pub timestamp: i64,

    // task data
    data: ShareLock<Vars>,

    /// task state
    state: ShareLock<TaskState>,

    /// task error
    err: ShareLock<Option<Error>>,

    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    // previous tid
    prev: ShareLock<Option<String>>,

    // next tid
    next: ShareLock<Vec<String>>,

    // parent tid
    parent: ShareLock<Option<String>>,

    proc: Arc<Process>,

    node: Arc<Node>,

    runtime: Arc<Runtime>,
}

impl Task {
    pub fn new(proc: &Arc<Process>, tid: &str, node: Arc<Node>, rt: &Arc<Runtime>) -> Self {
        Self {
            pid: proc.id().to_string(),
            id: tid.to_string(),
            node,
            data: Arc::new(RwLock::new(Vars::new())),
            state: Arc::new(RwLock::new(TaskState::None)),
            err: Arc::new(RwLock::new(None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            prev: Arc::new(RwLock::new(None)),
            next: Arc::new(RwLock::new(Vec::new())),
            parent: Arc::new(RwLock::new(None)),
            timestamp: utils::time::timestamp(),
            proc: proc.clone(),
            runtime: rt.clone(),
        }
    }

    pub fn unique_id(&self) -> String {
        format!("{}:{}", self.pid, self.id)
    }

    pub fn proc(&self) -> &Arc<Process> {
        &self.proc
    }

    pub(crate) fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    pub fn node(&self) -> &Arc<Node> {
        &self.node
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

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }
        utils::time::time_millis() - self.start_time()
    }

    pub fn is_emit(&self) -> bool {
        let Some(v) = self.sign() else {
            return true;
        };

        let is_no_emit = (v & Sign::NO_EMIT) == Sign::NO_EMIT;
        !is_no_emit
    }

    pub fn set_emit(&self, v: bool) {
        if v {
            self.remove_sign(Sign::NO_EMIT);
        } else {
            self.set_sign(Sign::NO_EMIT);
        }
    }

    pub fn is_auto_complete(&self) -> bool {
        let Some(v) = self.sign() else {
            return true;
        };

        let is_no_automate = (v & Sign::NO_AUTO_COMPLETE) == Sign::NO_AUTO_COMPLETE;
        !is_no_automate
    }

    pub fn is_sign(&self, sign: Sign) -> bool {
        self.with_data(|data| {
            if let Some(ref v) = data.get::<Sign>(consts::TASK_SIGN) {
                return (*v & sign) == sign;
            }
            false
        })
    }

    pub fn sign(&self) -> Option<Sign> {
        self.with_data(|data| data.get::<Sign>(consts::TASK_SIGN))
    }

    pub fn set_sign(&self, sign: Sign) {
        self.set_data_with(move |data| {
            if let Some(ref v) = data.get::<Sign>(consts::TASK_SIGN) {
                data.set(consts::TASK_SIGN, *v | sign);
            } else {
                data.set(consts::TASK_SIGN, sign);
            }
        });
    }

    pub fn remove_sign(&self, sign: Sign) {
        self.set_data_with(move |data| {
            if let Some(ref v) = data.get::<Sign>(consts::TASK_SIGN) {
                data.set(consts::TASK_SIGN, *v & !sign);
            }
        });
    }
    pub fn set_auto_complete(&self, v: bool) {
        if v {
            self.remove_sign(Sign::NO_AUTO_COMPLETE);
        } else {
            self.set_sign(Sign::NO_AUTO_COMPLETE);
        }
    }
    pub fn create_context(self: &Arc<Self>) -> Context {
        self.proc.create_context(self)
    }

    pub fn create_message(self: &Arc<Self>) -> Message {
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

            // append act.params to inputs
            inputs.set(consts::ACT_PARAMS_KEY, self.params());
        }

        // append act.optins to inputs
        inputs.set(consts::ACT_OPTIONS_KEY, self.options());

        // append workflow model to inputs
        inputs.set(
            consts::WORKFLOW_MODEL_KEY,
            Vars::new()
                .with("id", workflow.id.clone())
                .with("name", workflow.name)
                .with("options", workflow.options),
        );

        // add error to inputs
        if let Some(err) = self.err() {
            inputs.set(consts::ACT_ERR_CODE, err.ecode);
            inputs.set(consts::ACT_ERR_MESSAGE, err.message);
        }

        let state: MessageState = self.state().into();
        Message {
            id: utils::longid(),
            tid: self.id.clone(),
            name: self.node.content.name(),
            r#type: self.node.kind().to_string(),
            state,
            pid: self.pid.clone(),
            nid: self.node.id().to_string(),
            mid: workflow.id.clone(),
            uses: self.node.uses(),
            inputs,
            outputs: self.outputs(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            retry_times: 0,
            timestamp: self.timestamp,
        }
    }

    pub fn prev_id(&self) -> Option<String> {
        let ret = self.prev.read().unwrap();
        ret.clone()
    }

    pub fn next_ids(&self) -> Vec<String> {
        let ret = self.next.read().unwrap();
        ret.clone()
    }

    pub fn parent_id(&self) -> Option<String> {
        let ret = self.parent.read().unwrap();
        ret.clone()
    }

    pub fn parent(&self) -> Option<Arc<Task>> {
        if let Some(parent) = self.parent.read().unwrap().clone()
            && let Some(task) = self.proc.task(&parent)
        {
            return Some(task.clone());
        }

        None
    }

    pub fn children(&self) -> Vec<Arc<Self>> {
        self.proc.children(&self.id)
    }

    pub fn next(&self) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        let nexts = self.next_ids();
        for tid in &nexts {
            if let Some(task) = self.proc.task(tid) {
                ret.push(task);
            }
        }
        ret
    }

    pub fn siblings(&self) -> Vec<Arc<Self>> {
        let mut ret = Vec::new();
        if let Some(parent) = self.parent() {
            let children = parent.children();
            ret.extend(children.iter().filter(|iter| iter.id != self.id).cloned());
        }

        ret
    }

    pub fn inputs(self: &Arc<Self>) -> Vars {
        let ctx = self.create_context();
        let mut inputs = Vars::new();
        if let Some(prev) = self.prev_id()
            && let Some(prev_task) = self.proc.task(&prev)
        {
            // set the prev task's outputs as current inputs
            for (ref k, v) in &prev_task.outputs() {
                inputs.set(k, v.clone());
            }
        }
        // merge the node vars
        let vars = utils::fill_inputs(&self.node.content.vars(), &ctx);
        inputs.extend(vars)
    }

    pub fn outputs(self: &Arc<Self>) -> Vars {
        let ctx = self.create_context();
        let mut outputs = Vars::new();
        let mut exposes = self.node.content.exposes().clone();
        if exposes.is_empty() {
            // fallback: check options for exposes (runtime push actions)
            if let Some(opt_exposes) = self.options().get::<Vec<Variant>>("exposes") {
                exposes = opt_exposes;
            }
        }
        if !exposes.is_empty() {
            for var in &exposes {
                outputs.set(&var.name, var.value.clone());
            }
        } else {
            // export all data except the private ones
            for (key, _) in ctx.task().data().iter() {
                if !consts::is_private_key(key) {
                    outputs.set(key, json!(null))
                }
            }
        }

        utils::fill_outputs(&outputs, &ctx)
    }

    pub fn options(self: &Arc<Self>) -> Vars {
        self.node.content.options()
    }

    pub fn params(self: &Arc<Self>) -> serde_json::Value {
        let ctx = self.create_context();
        utils::fill_params(&self.node.content.params(), &ctx)
    }

    pub fn set_prev(&self, prev: &str) {
        *self.prev.write().unwrap() = Some(prev.to_string());
    }

    pub fn set_parent(&self, parent: &str) {
        *self.parent.write().unwrap() = Some(parent.to_string());
    }

    pub fn set_next(&self, next: &str) {
        self.next.write().unwrap().push(next.to_string());
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            self.set_end_time(utils::time::time_millis());

            if self.id == TASK_ROOT_TID {
                self.proc().set_state(state.clone());
            }
        } else if state.is_created() {
            self.set_start_time(utils::time::time_millis());
        }
        *self.state.write().unwrap() = state.clone();

        // clean the err
        if state != TaskState::Error {
            *self.err.write().unwrap() = None;
        }
    }

    pub fn set_err(&self, err: &Error) {
        *self.err.write().unwrap() = Some(err.clone());

        self.set_data_with(|data| {
            data.set(consts::ACT_ERR_CODE, &err.ecode);
            data.set(consts::ACT_ERR_MESSAGE, &err.message)
        });
        self.set_state(TaskState::Error);
    }

    pub fn clear_err_with(&self, new_state: TaskState) {
        *self.err.write().unwrap() = None;
        self.set_data_with(|data| {
            data.remove(consts::ACT_ERR_CODE);
            data.remove(consts::ACT_ERR_MESSAGE);
        });
        self.set_state(new_state);
    }

    pub(crate) fn set_pure_err(&self, err: &Error) {
        *self.err.write().unwrap() = Some(err.clone());
    }

    pub fn err(&self) -> Option<Error> {
        self.err.read().unwrap().clone()
    }

    pub fn set_pure_state(&self, state: TaskState) {
        *self.state.write().unwrap() = state;
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

    pub fn is_uses(&self, v: &str) -> bool {
        if self.node.kind() == NodeKind::Act {
            return self.node.uses().as_deref() == Some(v);
        }
        false
    }

    pub fn is_timeouts(&self) -> bool {
        match &self.node.content {
            NodeContent::Step(step) => !step.timeouts.is_empty(),
            _ => false,
        }
    }

    pub fn is_catches(&self) -> bool {
        match &self.node.content {
            NodeContent::Step(step) => !step.catches.is_empty(),
            _ => false,
        }
    }

    pub fn exec(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        // let _lock = self.sync.lock().unwrap();
        debug!("exec task={:?}", ctx.task());
        if self.state().is_completed() {
            return Err(ActError::Runtime(format!(
                "task({}:{}) is already completed",
                self.pid, self.id
            )));
        }
        self.init(ctx)?;
        self.run(ctx)?;
        self.next(ctx)?;
        Ok(())
    }

    pub fn update(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        debug!("update task={:?}", ctx.task());
        let action = ctx.action().ok_or(ActError::Action(
            "cannot find action in context".to_string(),
        ))?;

        match action.event {
            EventAction::Push => {
                let package = ctx.get_var::<String>("uses").unwrap_or_default();
                let act = Act {
                    id: ctx.get_var::<String>("id").unwrap_or_default(),
                    name: ctx.get_var::<String>("name").unwrap_or_default(),
                    desc: ctx.get_var::<String>("desc").unwrap_or_default(),
                    r#if: ctx.get_var::<String>("if"),
                    vars: ctx.get_var::<Vec<Variant>>("vars").unwrap_or_default(),
                    uses: package.clone(),
                    params: ctx.get_var("params").unwrap_or_default(),
                    options: ctx.get_var("options").unwrap_or_default(),
                    exposes: ctx.get_var("exposes").unwrap_or_default(),
                    ..Default::default()
                };

                // check key property
                if package.is_empty() {
                    return Err(crate::ActError::Action(
                        "cannot find 'uses' in act".to_string(),
                    ));
                }

                ctx.dispatch_act(&act, Vars::new())?;
            }
            EventAction::Remove => {
                self.set_state(TaskState::Removed);
                self.next(ctx)?;
            }
            EventAction::Submit => {
                self.update_data(&ctx.vars());
                self.set_state(TaskState::Submitted);
                self.next(ctx)?;
            }
            EventAction::Next => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}:{}' is already completed",
                        self.pid, self.id
                    )));
                }
                self.update_data(&ctx.vars());
                self.set_state(TaskState::Completed);
                self.next(ctx)?;
            }
            EventAction::Back => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}:{}' is already completed",
                        self.pid, self.id
                    )));
                }
                let nid = ctx
                    .get_var::<String>(consts::ACT_TO)
                    .ok_or(ActError::Action(
                        "cannot find 'to' value in options".to_string(),
                    ))?;

                let mut path_tasks = Vec::new();
                let task = self.backs(
                    &|t| t.node.kind() == NodeKind::Step && t.node.id() == nid,
                    &mut path_tasks,
                );

                let task = task.ok_or(ActError::Action(format!(
                    "cannot find history task by nid '{nid}'",
                )))?;

                ctx.back_task(&ctx.task(), &path_tasks)?;
                ctx.redo_task(&task)?;
            }
            EventAction::Cancel => {
                // find the parent step task
                let mut step = ctx.task().parent();
                while let Some(task) = &step {
                    if task.is_kind(NodeKind::Step) {
                        break;
                    }
                    step = task.parent();
                }

                let task = step.ok_or(ActError::Action(format!(
                    "cannot find parent step task by tid '{}'",
                    ctx.task().id,
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
                if nexts.is_empty() {
                    return Err(ActError::Action("cannot find cancelled tasks".to_string()));
                }

                // mark the path tasks as completed
                for p in path_tasks {
                    if p.state().is_running() {
                        p.set_state(TaskState::Completed);
                        ctx.emit_task(&p)?;
                    } else if p.state().is_pending() {
                        p.set_state(TaskState::Skipped);
                        ctx.emit_task(&p)?;
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
                        "task '{}:{}' is already completed",
                        self.pid, self.id
                    )));
                }
                ctx.abort_task(&ctx.task())?;
            }
            EventAction::Skip => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}:{}' is already completed",
                        self.pid, self.id
                    )));
                }

                for task in self.siblings() {
                    if task.state().is_completed() {
                        continue;
                    }
                    task.set_state(TaskState::Skipped);
                    ctx.emit_task(&task)?;
                }

                // set both current act and parent step to skip
                self.set_state(TaskState::Skipped);
                self.next(ctx)?;
            }
            EventAction::Error => {
                let ecode = ctx
                    .get_var::<String>(consts::ACT_ERR_CODE)
                    .ok_or(ActError::Action(format!(
                        "cannot find '{}' in options",
                        consts::ACT_ERR_CODE
                    )))?;

                let error = ctx
                    .get_var::<String>(consts::ACT_ERR_MESSAGE)
                    .unwrap_or("".to_string());

                let err = Error::new(&error, &ecode);
                debug!("error: {err:?}");
                let task = &ctx.task();
                if task.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}:{}' is already completed",
                        task.pid, task.id
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
                    sub.set_state(TaskState::Skipped);
                    ctx.emit_task(sub)?;
                }
                task.set_err(&err);
                task.set_data(&ctx.vars());
                task.on_error(ctx)?;
            }
            EventAction::SetProcessVars => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}:{}' is already completed",
                        self.pid, self.id
                    )));
                }

                self.proc.set_data(&ctx.vars());
                // emit the task change (issue #)
                ctx.emit_task(self)?;
            }
        };

        if action.event != EventAction::Push {
            // update the message status after doing action
            ctx.runtime.cache().store().set_message_with(
                &action.pid,
                &action.tid,
                MessageStatus::Completed,
            )?;
        }
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        match &self.node.content {
            NodeContent::Branch(n) => {
                let siblings = self.siblings();
                if !n.needs.is_empty() {
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
                        self.set_state(TaskState::Skipped);
                    }
                }

                false
            }
            _ => true,
        }
    }

    pub async fn resume(self: &Arc<Self>, ctx: &Context) -> Result<()> {
        if self.is_ready() {
            self.set_state(TaskState::Running);
            ctx.runtime.emitter().emit_task_event(self)?;
            self.exec(ctx)?;
        }

        Ok(())
    }

    pub fn into_data(self: &Arc<Self>) -> Result<data::Task> {
        let id = utils::Id::new(&self.pid, &self.id);
        Ok(data::Task {
            id: id.id(),
            prev: self.prev_id(),
            next: self.next_ids(),
            parent: self.parent_id(),
            name: self.node.content.name(),
            kind: self.node.kind().to_string(),
            pid: self.pid.clone(),
            tid: self.id.clone(),
            node_data: self.node.to_string(),
            state: self.state().into(),
            data: self.data().to_string(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            timestamp: self.timestamp,
            err: self.err().map(|err| err.to_string()),
            v: data::Task::version(),
        })
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

        let mut prev = self.prev_id();
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

                prev = task.prev_id();
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
        let nexts = self.next();
        if !nexts.is_empty() {
            for task in &nexts {
                if predicate(task) {
                    ret.push(task.clone());
                } else {
                    // push the path tasks
                    if task.state().is_running() || task.state().is_pending() {
                        path.push(task.clone());
                    }

                    // find the next follows
                    ret.extend(task.follows(predicate, path));
                }
            }
        }

        ret
    }

    pub fn is_next(&self) -> bool {
        let state = self.state();
        state.is_completed() || state.is_interrupted()
    }

    pub fn check_uses_action(&self, ctx: &Context) -> Result<NextAction> {
        let task = ctx.task();
        if task.state().is_running()
            && task.node().kind() == NodeKind::Step
            && task.node().uses().is_some()
            && !self.is_sign(Sign::USES_COMPLETE)
        {
            let mut count = 0;
            let task_children = self.children();
            let task_children = task_children
                .iter()
                .filter(|t| t.node().kind() == NodeKind::Act)
                .collect::<Vec<_>>();

            for task in task_children.iter() {
                if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Ready);
                    self.runtime.emitter().emit_task_event(task)?;
                    task.exec(ctx)?;
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count != task_children.len() {
                return Ok(NextAction::Stop);
            }

            // marked sign flag when all children task completed
            self.set_sign(Sign::USES_COMPLETE);
        }

        Ok(NextAction::Continue)
    }

    pub fn check_in_children(self: &Arc<Self>, ctx: &Context) -> Result<NextAction> {
        if self.state().is_running() {
            // run into children nodes if there is children nodes
            if !self.is_sign(Sign::IN_CHILDREN) {
                let children = self.node().children();
                if !children.is_empty() {
                    for child in &children {
                        ctx.sched_task(child, ctx.task())?;
                    }
                    self.set_sign(Sign::IN_CHILDREN);
                    return Ok(NextAction::Stop);
                }
            }
        }

        Ok(NextAction::Continue)
    }

    pub fn auto_complete(self: &Arc<Self>, ctx: &Context) -> Result<NextAction> {
        let state = self.state();

        if state.is_running() {
            let task_children = self.children();
            let mut count = 0;

            // for msg act, the client can only receive 'completed' message
            if self.node().kind() == NodeKind::Act
                && let Some(run_as) = ctx
                    .task()
                    .with_data(|data| data.get::<ActRunAs>(consts::ACT_RUN_AS))
                && run_as == ActRunAs::Msg
            {
                self.set_state(TaskState::Completed);
            }

            for task in task_children.iter() {
                if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Ready);
                    self.runtime.emitter().emit_task_event(task)?;
                    task.exec(ctx)?;
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == task_children.len()
                && self.is_auto_complete()
                && !self.state().is_completed()
            {
                // check if the task is error catched
                let is_empty_catched = task_children
                    .iter()
                    .filter(|t| t.is_sign(Sign::CATCH))
                    .all(|t| t.state().is_skip());

                if self.is_sign(Sign::ERROR) && is_empty_catched {
                    // no any action to match
                    // resume the task error state
                    let err = self.with_data(|data| {
                        Error::new(
                            &data
                                .get::<String>(consts::ACT_ERR_MESSAGE)
                                .unwrap_or_default(),
                            &data.get::<String>(consts::ACT_ERR_CODE).unwrap_or_default(),
                        )
                    });
                    self.set_err(&err);
                    ctx.emit_error()?;
                    return Ok(NextAction::Stop);
                } else {
                    self.set_state(TaskState::Completed);
                }
            }
        }

        // continue to run next
        Ok(NextAction::Continue)
    }
}

impl ActTask for Arc<Task> {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.set_task(self);
        if ctx.task().state().is_none() {
            ctx.prepare();
            ctx.task().set_state(TaskState::Ready);
            match &self.node.content {
                NodeContent::Workflow(workflow) => workflow.init(ctx)?,
                NodeContent::Branch(branch) => branch.init(ctx)?,
                NodeContent::Step(step) => step.init(ctx)?,
                NodeContent::Act(act) => act.init(ctx)?,
            }
            ctx.emit_task(&ctx.task())?;
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if task.state().is_ready() {
            task.set_state(TaskState::Running);
            match &self.node.content {
                NodeContent::Workflow(workflow) => workflow.run(ctx),
                NodeContent::Branch(branch) => branch.run(ctx),
                NodeContent::Step(step) => step.run(ctx),
                NodeContent::Act(act) => act.run(ctx),
            }?;

            ctx.emit_task(&ctx.task())?;
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<NextAction> {
        ctx.set_task(self);
        let task = ctx.task();

        // 1. check uses action completed
        let mut next_action = task.check_uses_action(ctx)?;

        // 2. check run into children
        if next_action.is_continue() {
            next_action = task.check_in_children(ctx)?;
        }

        // 3. auto-complete task state
        if next_action.is_continue() {
            next_action = task.auto_complete(ctx)?;
        }

        // 4. schedule next task
        if next_action.is_continue() && task.is_next() {
            next_action = match &self.node.content {
                NodeContent::Workflow(data) => data.next(ctx)?,
                NodeContent::Step(data) => data.next(ctx)?,
                NodeContent::Branch(data) => data.next(ctx)?,
                NodeContent::Act(data) => data.next(ctx)?,
            };
        }

        debug!(
            "next action:{} node={:?} task={:?}",
            next_action,
            ctx.task().node,
            ctx.task()
        );

        if task.state().is_completed() {
            ctx.emit_task(self)?;
        }

        // 5. move to parent and continue
        if next_action.is_parent() {
            let parent = task.parent();
            if let Some(p) = &parent.clone() {
                let outputs = task.outputs();
                // Update the parent task's data with current task's outputs
                p.update_data(&outputs);
                return p.next(ctx);
            }
        }

        Ok(NextAction::Continue)
    }

    fn on_error(&self, ctx: &Context) -> Result<()> {
        ctx.set_task(self);
        match &self.node.content {
            NodeContent::Workflow(data) => data.on_error(ctx),
            NodeContent::Step(data) => data.on_error(ctx),
            NodeContent::Branch(data) => data.on_error(ctx),
            NodeContent::Act(data) => data.on_error(ctx),
        }
    }

    fn on_timeout(&self, ctx: &Context) -> Result<()> {
        ctx.set_task(self);
        match &self.node.content {
            NodeContent::Workflow(data) => data.on_timeout(ctx),
            NodeContent::Step(data) => data.on_timeout(ctx),
            NodeContent::Branch(data) => data.on_timeout(ctx),
            NodeContent::Act(data) => data.on_timeout(ctx),
        }
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("name", &self.node.name())
            .field("type", &self.node.kind())
            .field("pid", &self.pid)
            .field("nid", &self.node.id())
            .field("state", &self.state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("prev", &self.prev_id())
            .field("next", &self.next_ids())
            .field("parent", &self.parent_id())
            .field("data", &self.data())
            .field("err", &self.err())
            .finish()
    }
}

impl Task {
    pub fn data(&self) -> Vars {
        self.data.read().unwrap().clone()
    }

    pub fn vars(&self) -> Vars {
        let mut vars = self.data();
        if let Some(parent) = self.parent() {
            let data = parent.vars();
            vars = vars.extend(data)
        }

        vars
    }

    pub fn with_data<T, F: Fn(&Vars) -> T>(&self, f: F) -> T {
        let data = self.data.read().unwrap();
        f(&data)
    }

    pub fn set_data_with<F: Fn(&mut Vars)>(&self, f: F) {
        let mut data = self.data.write().unwrap();
        f(&mut data)
    }

    pub fn set_data(&self, vars: &Vars) {
        let mut data = self.data.write().unwrap();
        for (ref name, value) in vars {
            data.set(name, value);
        }
    }

    pub fn update_data_if_exists<F: Fn(&mut Vars) -> bool>(&self, f: F) -> bool {
        let mut data = self.data.write().unwrap();
        f(&mut data)
    }

    pub fn find<T>(&self, name: &str) -> Option<T>
    where
        T: DeserializeOwned + std::fmt::Debug + Clone,
    {
        let result = self.with_data(move |data| data.get(name));
        if result.is_some() {
            return result;
        }

        let mut parent = self.parent();
        while let Some(task) = parent {
            let result = task.with_data(|data| data.get::<T>(name));
            if result.is_some() {
                return result;
            }
            parent = task.parent();
        }
        None
    }

    pub fn update_data(&self, vars: &Vars) {
        let mut refs = Vec::new();
        let mut parent = self.parent();
        while let Some(task) = parent {
            refs.push(task.clone());
            parent = task.parent();
        }

        for (ref name, ref value) in vars {
            // skip private keys
            if consts::is_private_key(name) {
                continue;
            }
            for t in refs.iter().rev() {
                let is_updated = t.update_data_if_exists(|v| {
                    if v.contains_key(name) {
                        v.set(name, value);
                        return true;
                    }
                    false
                });

                if is_updated {
                    break;
                }
            }
        }

        // also set the to current task
        self.set_data(vars);
    }

    fn move_next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        if let Some(next) = &task.node.next().upgrade() {
            ctx.sched_task(next, ctx.task())?;
            return Ok(true);
        }

        Ok(false)
    }
}
