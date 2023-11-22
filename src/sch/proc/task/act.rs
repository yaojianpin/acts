mod catch;
mod r#for;
mod rule;
mod r#use;

#[cfg(test)]
mod tests;

use crate::{
    event::ActionState,
    model::Act,
    sch::{Context, TaskState},
    ActTask, ActUse, Result,
};
use async_trait::async_trait;
use rule::Rule;

#[async_trait]
impl ActTask for Act {
    fn init(&self, ctx: &Context) -> Result<()> {
        if self.needs.len() > 0 {
            ctx.task.set_state(TaskState::Pending);
        } else {
            if let Some(ref f) = self.r#for {
                return f.init(ctx);
            }

            if let Some(ref s) = self.r#use {
                return ActUse::new(s).init(ctx);
            }

            ctx.task.set_state(TaskState::Interrupt);
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if let Some(ref f) = self.r#for {
            return f.run(ctx);
        }

        if let Some(ref s) = self.r#use {
            return ActUse::new(s).run(ctx);
        }

        // when resuming the pending task, the current state is running
        // for general act task, reset the state to interrupt
        if ctx.task.state().is_running() {
            ctx.task.set_state(TaskState::Interrupt);
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

        let state = ctx.task.state();
        if state.is_running() {
            let tasks = ctx.task.children();

            let mut ok_count = 0;
            for task in tasks.iter() {
                if task.state().is_error() {
                    ctx.set_err(&task.state().as_err().unwrap_or_default());
                    ctx.emit_error()?;
                    return Ok(false);
                }
                if task.state().is_skip() {
                    ctx.task.set_action_state(ActionState::Skipped);
                    return Ok(true);
                }

                if task.state().is_success() {
                    ok_count += 1;
                }
            }

            if ok_count == tasks.len() {
                if !ctx.task.state().is_completed() {
                    ctx.task.set_action_state(ActionState::Completed);
                }
            }
        }

        return Ok(true);
    }

    fn error(&self, ctx: &Context) -> Result<()> {
        self.catches.error(ctx)?;
        if ctx.task.state().is_error() {
            return ctx.emit_error();
        }
        return Ok(());
    }
}
