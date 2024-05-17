use super::TaskLifeCycle;
use crate::{
    sch::{Context, TaskState},
    ActTask, Req, Result,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Req {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        for s in self.on_created.iter() {
            task.add_hook_stmts(TaskLifeCycle::Created, s);
        }
        for s in self.on_completed.iter() {
            task.add_hook_stmts(TaskLifeCycle::Completed, s);
        }

        for s in self.catches.iter() {
            task.add_hook_catch(TaskLifeCycle::ErrorCatch, s);
        }

        if self.timeout.len() > 0 {
            for s in &self.timeout {
                task.add_hook_timeout(TaskLifeCycle::Timeout, s);
            }
        }

        task.set_state(TaskState::Interrupt);
        Ok(())
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            let tasks = task.children();

            let mut count = 0;
            for t in tasks.iter() {
                if t.state().is_error() {
                    ctx.emit_error()?;
                    return Ok(false);
                }
                if t.state().is_skip() {
                    task.set_state(TaskState::Skipped);
                    return Ok(true);
                }

                if t.state().is_success() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }
            }
        }

        return Ok(true);
    }
}
