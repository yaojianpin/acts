use crate::sch::ActTask;
use crate::{debug, sch::proc::Task, utils, Act, Context, TaskState, Vars};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

impl_act_state!(Act);
impl_act_time!(Act);
impl_act_id!(Act);

impl Act {
    pub fn new(step_task_id: &str, owner: &str) -> Self {
        let tid = utils::shortid();
        Self {
            id: tid.to_string(),
            owner: owner.to_string(),

            state: Arc::new(RwLock::new(TaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            env: Arc::new(RwLock::new(HashMap::new())),
            user: Arc::new(RwLock::new(None)),
            step_task_id: step_task_id.to_string(),
        }
    }

    pub(in crate::sch) fn check_pass(&self, _ctx: &Context) -> bool {
        true
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

    pub(in crate::sch) fn complete(&self, user: &str) {
        debug!("complete: {}, user:{}", self.id, user);
        *self.user.write().unwrap() = Some(user.to_string());
        self.set_state(&TaskState::Success);
    }

    pub(in crate::sch) fn parent(&self, ctx: &Context) -> Option<Task> {
        if let Some(node) = ctx.proc.tree.node(&self.step_task_id) {
            return Some(node.data());
        }
        None
    }
}

impl ActTask for Act {
    fn run(&self, ctx: &Context) {
        let user = ctx.user();
        self.complete(&user.unwrap());
    }
}
