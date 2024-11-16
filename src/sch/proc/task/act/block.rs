use crate::{sch::Context, ActTask, Block, Result, TaskState};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Block {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.task().set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        for (key, value) in &self.inputs {
            ctx.set_var(&key, value);
        }
        for s in self.then.iter() {
            s.exec(ctx)?;
        }
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        let mut is_next: bool = false;
        if state.is_running() {
            let tasks = task.children();
            let mut count = 0;

            for task in tasks.iter() {
                if task.state().is_none() || task.state().is_running() {
                    is_next = true;
                } else if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.runtime.scher().emit_task_event(task)?;

                    task.exec(ctx)?;
                    is_next = true;
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }

                if let Some(next) = &self.next {
                    ctx.append_act(next)?;
                    return Ok(true);
                }
            }
        } else if state.is_skip() {
            if let Some(next) = &self.next {
                ctx.append_act(next)?;
                return Ok(true);
            }
        }
        Ok(is_next)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            let tasks = task.children();

            let mut count = 0;
            for task in tasks.iter() {
                if task.state().is_error() {
                    ctx.emit_error()?;
                    return Ok(false);
                }
                if task.state().is_skip() {
                    task.set_state(TaskState::Skipped);
                    return Ok(true);
                }

                if task.state().is_success() {
                    count += 1;
                }
            }
            if count == tasks.len() {
                if !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }

                if let Some(next) = &self.next {
                    ctx.append_act(next)?;
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }
}
