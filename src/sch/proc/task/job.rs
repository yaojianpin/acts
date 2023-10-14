use crate::{
    event::ActionState,
    model::Job,
    sch::{Context, TaskState},
    ActTask, Result,
};
use async_trait::async_trait;
use core::clone::Clone;
use std::{ops::Deref, sync::Arc};

#[async_trait]
impl ActTask for Job {
    fn init(&self, ctx: &Context) -> Result<()> {
        if self.needs.len() > 0 {
            ctx.task.set_state(TaskState::Pending);
        }
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let children = ctx.task.node.children();
        if children.len() > 0 {
            for child in &ctx.task.node.children() {
                ctx.sched_task(child);
            }
        } else {
            ctx.task.set_action_state(ActionState::Completed);
        }
        Ok(children.len() > 0)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        if state.is_running() {
            ctx.task.set_action_state(ActionState::Completed);
            return Ok(true);
        }

        Ok(false)
    }
}

impl From<Arc<Job>> for Job {
    fn from(item: Arc<Job>) -> Self {
        item.deref().clone()
    }
}
