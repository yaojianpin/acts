use crate::{
    event::ActionState,
    model::Step,
    sch::{Context, TaskState},
    ActTask, Result,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Step {
    fn init(&self, ctx: &Context) -> Result<()> {
        if let Some(expr) = &self.r#if {
            let cond = ctx.eval(expr)?;
            if !cond {
                ctx.task.set_action_state(ActionState::Skipped);
                return Ok(());
            }
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if let Some(script) = &self.run {
            ctx.run(script)?;
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        if state.is_running() {
            let children = ctx.task.node.children();
            let mut is_next = false;
            if children.len() > 0 {
                for child in &ctx.task.node.children() {
                    ctx.sched_task(child);
                }
                is_next = true;
            } else if let Some(next) = ctx.task.node.next().upgrade() {
                ctx.task.set_action_state(ActionState::Completed);
                ctx.sched_task(&next);
                is_next = true;
            } else {
                ctx.task.set_action_state(ActionState::Completed);
            }
            return Ok(is_next);
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = ctx.task.node.next().upgrade() {
                ctx.sched_task(&next);
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        if state.is_running() {
            let tasks = ctx.task.children();
            let mut ok_count = 0;
            for task in tasks.iter() {
                if task.state().is_pending() && task.is_ready() {
                    let ctx = task.create_context(&ctx.scher);

                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.scher.emitter().emit_task_event(task);
                    task.exec(&ctx)?;
                    return Ok(false);
                } else if task.state().is_error() {
                    ctx.set_err(&task.state().as_err().unwrap_or_default());
                    ctx.emit_error();
                    return Ok(false);
                }

                // regard success and skip as completed
                if task.state().is_success() || task.state().is_skip() {
                    ok_count += 1;
                }
            }

            if ok_count == tasks.len() {
                if !ctx.task.state().is_completed() {
                    ctx.task.set_action_state(ActionState::Completed);
                }

                if let Some(next) = &ctx.task.node.next().upgrade() {
                    ctx.sched_task(next);
                    return Ok(false);
                }
                return Ok(true);
            }
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = ctx.task.node.next().upgrade() {
                ctx.sched_task(&next);
                return Ok(false);
            }
            return Ok(true);
        }

        return Ok(false);
    }
}
