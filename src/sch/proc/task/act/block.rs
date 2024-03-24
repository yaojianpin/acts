use crate::{event::ActionState, sch::Context, Act, ActTask, Block, Result, TaskState};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Block {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.task.set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        for (key, value) in &self.inputs {
            ctx.set_var(key, value);
        }
        for s in self.acts.iter() {
            s.exec(ctx)?;
        }
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        let mut is_next: bool = false;
        if state.is_running() {
            let tasks = ctx.task.children();
            let mut count = 0;

            for task in tasks.iter() {
                if task.state().is_none() || task.state().is_running() {
                    is_next = true;
                } else if task.state().is_pending() && task.is_ready() {
                    let ctx = task.create_context(&ctx.scher);

                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.scher.emitter().emit_task_event(task);
                    task.exec(&ctx)?;
                    is_next = true;
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !ctx.task.state().is_completed() {
                    ctx.task.set_action_state(ActionState::Completed);
                }

                if let Some(next) = &self.next {
                    ctx.append_act(&Act::Block(*next.clone()))?;
                    return Ok(true);
                }
            }
        } else if state.is_skip() {
            if let Some(next) = &self.next {
                ctx.append_act(&Act::Block(*next.clone()))?;
                return Ok(true);
            }
        }

        Ok(is_next)
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

                if let Some(next) = &self.next {
                    ctx.append_act(&Act::Block(*next.clone()))?;
                    return Ok(false);
                }
            }
        }

        return Ok(true);
    }
}
