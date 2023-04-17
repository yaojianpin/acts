use crate::sch::ActTask;
use crate::{
    sch::{proc::Task, Context},
    utils, Act, TaskState, Vars,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

impl Act {
    pub fn new(step_id: &str, owner: &str) -> Self {
        let tid = utils::shortid();
        Self {
            id: tid.to_string(),
            owner: owner.to_string(),

            env: Arc::new(RwLock::new(HashMap::new())),
            user: Arc::new(RwLock::new(None)),
            step_id: step_id.to_string(),
        }
    }

    pub fn env(&self) -> Vars {
        self.env.read().unwrap().clone()
    }

    pub fn user(&self) -> Option<String> {
        self.user.read().unwrap().clone()
    }

    pub fn set_user(&self, user: &str) {
        *self.user.write().unwrap() = Some(user.to_string());
    }

    pub(in crate::sch) fn parent(&self, ctx: &Context) -> Option<Arc<Task>> {
        ctx.proc.task(&self.step_id)
    }
}

impl ActTask for Act {
    fn prepare(&self, ctx: &Context) {
        if ctx.task.state().is_none() {
            ctx.task.set_state(&TaskState::WaitingEvent);
        } else if ctx.task.state().is_waiting() && ctx.action().is_some() {
            ctx.task.set_state(&TaskState::Running);
        }
    }

    fn run(&self, ctx: &Context) {
        if let Some(uid) = ctx.uid() {
            ctx.task.set_uid(&uid);
        }
    }

    fn post(&self, ctx: &Context) {
        ctx.task.set_state(&TaskState::Success);
    }
}
