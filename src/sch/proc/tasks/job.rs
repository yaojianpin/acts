use crate::{
    model::Job,
    sch::{Context, TaskState},
    ActTask,
};
use async_trait::async_trait;
use core::clone::Clone;
use std::{ops::Deref, sync::Arc};

#[async_trait]
impl ActTask for Job {
    fn run(&self, _ctx: &Context) {}
    fn post(&self, ctx: &Context) {
        ctx.task.set_state(TaskState::Success);
    }
}

impl From<Arc<Job>> for Job {
    fn from(item: Arc<Job>) -> Self {
        item.deref().clone()
    }
}
