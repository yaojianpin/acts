use super::{utils::dispatcher::Dispatcher, Task};
use crate::{
    event::{EventAction, MessageKind},
    sch::{NodeData, Scheduler, TaskState},
    utils::{self, consts},
    ActError, ActResult, ActValue, Action, Context, Message, ShareLock, Vars,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tracing::instrument;

#[derive(Serialize, Deserialize, Clone, PartialEq, PartialOrd, Eq, Ord, Debug, Default)]
pub enum ActKind {
    #[default]
    Action = 0,
    Some,
    Candidate,
    User,
}

#[derive(Clone)]
pub struct Act {
    pub id: String,
    pub kind: ActKind,
    pub pid: String,
    pub tid: String,
    pub vars: Vars,
    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,
    active: ShareLock<bool>,
    pub(crate) on_events: HashMap<String, ActValue>,
    pub(crate) task: Arc<Task>,
}

impl std::fmt::Debug for Act {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Act")
            .field("aid", &self.id)
            .field("kind", &self.kind)
            .field("pid", &self.pid)
            .field("tid", &self.tid)
            .field("vars", &self.vars)
            .field("state", &self.state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("active", &self.active())
            .field("vars", &self.vars())
            .finish()
    }
}

impl std::fmt::Display for ActKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ActKind::Action => "action",
            ActKind::Candidate => "candidate",
            ActKind::Some => "rule",
            ActKind::User => "user",
        })
    }
}

impl From<&str> for ActKind {
    fn from(value: &str) -> Self {
        match value {
            "candidate" => ActKind::Candidate,
            "some" => ActKind::Some,
            "user" => ActKind::User,
            "action" | _ => ActKind::Action,
        }
    }
}

impl Act {
    pub fn new(task: &Arc<Task>, kind: ActKind, vars: &Vars) -> Arc<Self> {
        let aid = utils::shortid();
        Act::new_with_id(task, kind, &aid, vars)
    }

    pub fn new_with_id(task: &Arc<Task>, kind: ActKind, id: &str, vars: &Vars) -> Arc<Self> {
        let mut on_events = HashMap::new();
        if let NodeData::Step(step) = task.node.data() {
            if let Some(on) = step.on {
                on_events = on.act.clone();
            }
        }

        Arc::new(Self {
            id: id.to_string(),
            kind,
            pid: task.pid.clone(),
            tid: task.tid.clone(),
            vars: vars.clone(),
            state: Arc::new(RwLock::new(TaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            active: Arc::new(RwLock::new(false)),
            on_events: on_events.clone(),

            task: task.clone(),
        })
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

    pub fn vars(&self) -> &Vars {
        &self.vars
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
    }

    pub fn active(&self) -> bool {
        *self.active.read().unwrap()
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

    pub fn create_message(&self, event: &EventAction) -> Message {
        let task = &self.task;
        let workflow = task.proc.workflow();

        let mut vars = self.vars.clone();
        vars.extend(self.task.vars());

        // append the related vars
        for key in consts::ACT_VARS {
            if let Some(v) = self.task.env.get(key) {
                vars.insert(key.to_string(), v);
            }
        }

        // append ord rule to vars
        if let Some(v) = self.task.env.get(consts::RULE_ORD) {
            vars.insert(consts::RULE_ORD.to_string(), v);
        }

        Message {
            kind: MessageKind::Act(self.kind.clone()),
            event: event.clone(),
            mid: workflow.id.clone(),
            topic: workflow.topic.clone(),
            nkind: task.node.kind().to_string(),
            nid: task.nid(),
            pid: task.pid.clone(),
            tid: task.tid.clone(),
            key: Some(self.id.clone()),
            vars,
        }
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            *self.end_time.write().unwrap() = utils::time::time();
        } else if state.is_running() || state.is_waiting() {
            let mut time = self.start_time.write().unwrap();
            if *time == 0 {
                *time = utils::time::time();
            }
        }
        *self.state.write().unwrap() = state;
    }

    pub fn set_active(&self, active: bool) {
        *self.start_time.write().unwrap() = utils::time::time();
        *self.active.write().unwrap() = active;
    }

    pub fn create_context(&self, scher: &Arc<Scheduler>) -> Arc<Context> {
        self.task.create_context(scher)
    }

    #[instrument]
    pub fn exec(self: &Arc<Self>, ctx: &Context, action: &Action) -> ActResult<()> {
        let event = action.event.as_str().into();

        // check uid
        let _ = action
            .options
            .get("uid")
            .map(|v| v.as_str().unwrap().to_string())
            .ok_or(ActError::Action(format!("cannot find uid in options")))?;

        match event {
            EventAction::Update => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "action '{}' is already completed",
                        self.id
                    )));
                }
                // just change vars, donot change state
            }
            EventAction::Complete => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "action '{}' is already completed",
                        self.id
                    )));
                }
                self.set_state(TaskState::Success);
            }
            EventAction::Back => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "action '{}' is already completed",
                        self.id
                    )));
                }
                let to = action.options.get("to").ok_or(ActError::Action(format!(
                    "cannot find to value in options",
                )))?;
                let nid = to.as_str().ok_or(ActError::Action(format!(
                    "to '{to}' value is not a valid string type",
                )))?;
                let task = ctx
                    .proc
                    .last_task_by_nid(nid)
                    .ok_or(ActError::Action(format!(
                        "cannot find history task by nid '{}'",
                        nid
                    )))?;

                ctx.back_task(&ctx.task, &self.id)?;
                ctx.redo_task(&task)?;
            }
            EventAction::Cancel => {
                if !self.state().is_success() {
                    return Err(ActError::Action(format!(
                        "act('{}') is not allowed to cancel",
                        self.id
                    )));
                }

                // check current task state
                // if current task is completed this action should cancel next tasks and create new task for this step
                // or just create a new act
                if self.task.state().is_completed() {
                    let nexts = ctx.proc.find_next_tasks(&ctx.task.tid);
                    for n in nexts {
                        ctx.undo_task(&n)?;
                    }
                    ctx.redo_task(&ctx.task)?;
                } else {
                    ctx.redo_act(self)?;
                }
            }
            EventAction::Abort => {
                if self.state().is_completed() {
                    return Err(ActError::Action(format!(
                        "action '{}' is already completed",
                        self.id
                    )));
                }
                ctx.abort_task(&ctx.task, &self.id)?;
            }
            name => {
                return Err(ActError::Action(
                    format!("action '{name}' is not  support",),
                ));
            }
        }

        // append action vars after executing the action
        ctx.set_action_vars(action)?;

        Ok(())
    }

    #[instrument]
    pub fn next(&self, ctx: &Context) -> ActResult<()> {
        let dispatcher = Dispatcher::new(ctx);
        let is_finished = dispatcher.next()?;
        if is_finished {
            ctx.task.next(ctx);
        }

        Ok(())
    }

    #[instrument]
    pub fn post(self: &Arc<Self>, ctx: &Context) -> ActResult<()> {
        if let Some(action) = &ctx.action() {
            ctx.dispatch_act(self, action.clone());
        }

        Ok(())
    }

    pub fn on_event(&self, event: &str, ctx: &Context) {
        if let Some(event) = self.on_events.get(event) {
            if event.is_string() {
                let ret = ctx.run(event.as_str().unwrap());
                if !self.state().is_error() && ret.is_err() {
                    self.set_state(TaskState::Fail(ret.err().unwrap().into()));
                }
            }
        }
    }
}
