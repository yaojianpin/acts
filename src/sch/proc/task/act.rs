mod r#for;
mod rule;

#[cfg(test)]
mod tests;

use crate::{
    model::Act,
    sch::{Context, TaskState},
    ActTask, Result,
};
use async_trait::async_trait;
use rule::Rule;

#[async_trait]
impl ActTask for Act {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.task.set_state(TaskState::Pending);
        if let Some(ref f) = self.r#for {
            f.init(ctx)?;
        }
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if let Some(ref f) = self.r#for {
            f.run(ctx)?;
        }
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        if let Some(ref f) = self.r#for {
            let is_next = f.next(ctx)?;
            return Ok(is_next);
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        if let Some(ref f) = self.r#for {
            let is_review = f.review(ctx)?;
            return Ok(is_review);
        }

        return Ok(true);
    }
}
