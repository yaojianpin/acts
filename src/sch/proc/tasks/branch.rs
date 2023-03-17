use crate::{
    model::Branch,
    sch::{Context, TaskState},
    ActError, ActTask,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Branch {
    fn run(&self, ctx: &Context) {
        if let Some(expr) = &self.r#if {
            match ctx.eval(expr) {
                Ok(cond) => {
                    if cond {
                        ctx.task.set_state(&TaskState::Success);
                    } else {
                        ctx.task.set_state(&TaskState::Skip);
                    }
                }
                Err(err) => ctx.task.set_state(&TaskState::Fail(err.into())),
            }
        } else {
            ctx.task
                .set_state(&TaskState::Fail(ActError::BranchIfError.into()));
        }
    }

    fn post(&self, ctx: &Context) {
        ctx.task.set_state(&TaskState::Success);
    }
}
