use super::TaskLifeCycle;
use crate::{
    event::ActionState,
    sch::{Context, TaskState},
    ActTask, Req, Result,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Req {
    fn init(&self, ctx: &Context) -> Result<()> {
        for s in self.on_created.iter() {
            ctx.task.add_hook_stmts(TaskLifeCycle::Created, s);
        }

        for s in self.on_completed.iter() {
            ctx.task.add_hook_stmts(TaskLifeCycle::Completed, s);
        }

        for s in self.on_error_catch.iter() {
            ctx.task.add_hook_catch(TaskLifeCycle::ErrorCatch, s);
        }
        ctx.task.set_state(TaskState::Interrupt);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        // when resuming the pending task, the current state is running
        // for general act task, reset the state to interrupt
        if ctx.task.state().is_running() {
            ctx.task.set_state(TaskState::Interrupt);
        }
        Ok(())
    }

    fn next(&self, _ctx: &Context) -> Result<bool> {
        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        if state.is_running() {
            let tasks = ctx.task.children();

            let mut count = 0;
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
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !ctx.task.state().is_completed() {
                    ctx.task.set_action_state(ActionState::Completed);
                }
            }
        }

        return Ok(true);
    }
}
