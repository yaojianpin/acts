mod act;
mod branch;
// mod hook;
mod step;
mod workflow;

use crate::Variant;
use crate::scheduler::tree::NodeOutputKind;
use crate::store::DbCollectionIden;
use crate::utils::consts::TASK_ROOT_TID;
use crate::{
    Act, ActError, ActTask, Error, Message, MessageState, NodeKind, Result, ShareLock, Vars,
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

    proc: Arc<Process>,

    node: Arc<Node>,

    // lifecycle hooks
    // hooks: ShareLock<HashMap<TaskLifeCycle, Vec<StatementBatch>>>,
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
            timestamp: utils::time::timestamp(),
            proc: proc.clone(),

            // hooks: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn is_emit_disabled(&self) -> bool {
        self.with_data(|data| data.get::<bool>(consts::TASK_EMIT_DISABLED))
            .unwrap_or(false)
    }

    pub fn set_emit_disabled(&self, v: bool) {
        self.set_data_with(move |data| {
            data.set(consts::TASK_EMIT_DISABLED, v);
        });
    }

    pub fn is_auto_complete(&self) -> bool {
        self.with_data(|data| data.get::<bool>(consts::TASK_AUOT_COMPLETE))
            .unwrap_or(true)
    }

    pub fn is_sign(&self, sign: &str) -> bool {
        self.with_data(|data| {
            if let Some(ref v) = data.get::<String>(consts::TASK_SIGN) {
                return v == sign;
            }
            false
        })
    }

    pub fn sign(&self) -> Option<String> {
        self.with_data(|data| data.get::<String>(consts::TASK_SIGN))
    }

    pub fn set_sign(&self, sign: &str) {
        self.set_data_with(move |data| {
            data.set(consts::TASK_SIGN, sign);
        });
    }
    pub fn set_auto_complete(&self, v: bool) {
        self.set_data_with(move |data| {
            data.set(consts::TASK_AUOT_COMPLETE, v);
        });
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
                .with("tag", workflow.tag),
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
            key: self.node.key(),
            uses: self.node.uses(),
            tag: self.node.tag().to_string(),
            inputs,
            outputs: self.outputs(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            retry_times: 0,
            timestamp: self.timestamp,
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

    pub fn inputs(self: &Arc<Self>) -> Vars {
        let ctx = self.create_context();
        let mut inputs = Vars::new();
        if let Some(prev) = self.prev()
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
        if let Some(exposes) = self
            .node
            .content
            .options()
            .get::<Vec<Variant>>(consts::ACT_EXPOSE)
        {
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

    pub fn set_prev(&self, prev: Option<String>) {
        *self.prev.write().unwrap() = prev;
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
            return self.node.uses() == v;
        }
        false
    }

    pub fn is_timeouts(&self) -> bool {
        match &self.node.content {
            NodeContent::Step(step) => !step.timeouts.is_empty(),
            NodeContent::Act(act) => !act.timeouts.is_empty(),
            _ => false,
        }
    }

    pub fn is_catches(&self) -> bool {
        match &self.node.content {
            NodeContent::Step(step) => !step.catches.is_empty(),
            NodeContent::Act(act) => !act.catches.is_empty(),
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
                let key = ctx.get_var::<String>("key").unwrap_or_default();
                let act = Act {
                    id: ctx.get_var::<String>("id").unwrap_or_default(),
                    name: ctx.get_var::<String>("name").unwrap_or_default(),
                    tag: ctx.get_var::<String>("tag").unwrap_or_default(),
                    key: key.clone(),
                    uses: package.clone(),
                    params: ctx.get_var("params").unwrap_or_default(),
                    options: ctx.get_var("options").unwrap_or_default(),
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
                    .get_var::<String>(consts::ACT_SUBFLOW_TO)
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
                task.error(ctx)?;
            }
            EventAction::SetProcessVars => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "task '{}:{}' is already completed",
                        self.pid, self.id
                    )));
                }

                self.proc.set_data(&ctx.vars());
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

    pub async fn run_hooks_timeout(&self, ctx: &Context) -> Result<()> {
        let children = self.node.children_in(NodeOutputKind::Timeout);
        if !children.is_empty() {
            self.set_sign(consts::TASK_SIGN_TIMEOUTS);
            let mut timeouts = self
                .with_data(|data| data.get::<Vec<String>>(consts::TASK_TIMEOUTS))
                .unwrap_or_default();
            let cost = self.cost();
            for child in &children {
                if timeouts.contains(&child.id) {
                    continue;
                }
                let mut is_timeout = false;
                if let Some(expr) = &child.content.r#if() {
                    is_timeout = ctx.eval::<bool>(expr)?;
                }
                if is_timeout {
                    timeouts.push(child.id.clone());
                    self.set_data_with(|data| data.set(consts::TASK_TIMEOUTS, timeouts.clone()));
                    ctx.sched_task_with_vars(
                        child,
                        Vars::new()
                            .with(consts::TASK_COST, cost)
                            .with(consts::TASK_SIGN, consts::TASK_SIGN_CATCH),
                    )?;
                }
            }
        }

        Ok(())
    }

    pub fn run_hooks_err(&self, ctx: &Context) -> Result<()> {
        if self.sign().is_some() {
            return Ok(());
        }
        let children = self.node.children_in(NodeOutputKind::Catch);
        if !children.is_empty() {
            // no if condition in catch acts
            let mut catch_elses = vec![];
            let mut is_err_catch = false;
            for child in children.iter() {
                let mut is_cond_catch = false;
                if let Some(expr) = &child.content.r#if() {
                    is_cond_catch = ctx.eval::<bool>(expr)?;
                } else {
                    catch_elses.push(child.clone());
                }

                if is_cond_catch {
                    is_err_catch = true;
                    self.set_sign(consts::TASK_SIGN_ERR);
                    self.set_state(TaskState::Running);
                    ctx.sched_task_with_vars(
                        child,
                        Vars::new().with(consts::TASK_SIGN, consts::TASK_SIGN_CATCH),
                    )?;
                }
            }

            if !is_err_catch && !catch_elses.is_empty() {
                self.set_sign(consts::TASK_SIGN_ERR);
                self.set_state(TaskState::Running);
                // if there is no other catched acts
                // do the none if action
                for child in catch_elses.iter() {
                    ctx.sched_task_with_vars(
                        child,
                        Vars::new().with(consts::TASK_SIGN, consts::TASK_SIGN_CATCH),
                    )?;
                }
            }
        }

        Ok(())
    }

    pub fn into_data(self: &Arc<Self>) -> Result<data::Task> {
        let id = utils::Id::new(&self.pid, &self.id);
        Ok(data::Task {
            id: id.id(),
            prev: self.prev(),
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
        if !children.is_empty() {
            for task in &children {
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

            if !self.state().is_completed() {
                ctx.emit_task(&ctx.task())?;
            }
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

    fn next(&self, ctx: &Context) -> Result<bool> {
        ctx.set_task(self);
        let mut is_next = false;
        if ctx.task().state().is_next() {
            is_next = match &self.node.content {
                NodeContent::Workflow(data) => data.next(ctx)?,
                NodeContent::Step(data) => data.next(ctx)?,
                NodeContent::Branch(data) => data.next(ctx)?,
                NodeContent::Act(data) => data.next(ctx)?,
            };
        }
        debug!("is_next:{} task={:?}", is_next, ctx.task());
        if self.state().is_completed() {
            self.update_data(&ctx.vars());
            ctx.emit_task(self)?;

            if !is_next {
                let parent = ctx.task().parent();
                if let Some(task) = &parent.clone() {
                    task.review(ctx)?;
                }
            }
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        if ctx.task().is_sign(consts::TASK_SIGN_TIMEOUTS) {
            return Ok(false);
        }
        // last task's outputs
        // update prev outputs to current task
        let outputs = ctx.task().outputs();
        self.update_data(&outputs);

        ctx.set_task(self);

        let before_state = self.state();
        let is_review = match &self.node.content {
            NodeContent::Workflow(data) => data.review(ctx)?,
            NodeContent::Step(data) => data.review(ctx)?,
            NodeContent::Branch(data) => data.review(ctx)?,
            NodeContent::Act(data) => data.review(ctx)?,
        };

        debug!("is_review:{} task={:?}", is_review, ctx.task());
        if self.state().is_completed() && before_state != self.state() {
            ctx.emit_task(self)?;
        }

        if is_review {
            let parent = ctx.task().parent();
            if let Some(task) = &parent.clone() {
                return task.review(ctx);
            }
        }

        Ok(false)
    }

    fn error(&self, ctx: &Context) -> Result<()> {
        ctx.set_task(self);
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
            .field("pid", &self.pid)
            .field("nid", &self.node.id())
            .field("state", &self.state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("prev", &self.prev())
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

    // pub fn expose(&self, keys: &Vec<&str>) {
    //     self.set_data_with(move |data| {
    //         data.set(
    //             consts::ACT_EXPOSE,
    //             keys.iter()
    //                 .filter(|v| !consts::is_private_key(v))
    //                 .collect::<Vec<_>>(),
    //         )
    //     });
    // }

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
}
