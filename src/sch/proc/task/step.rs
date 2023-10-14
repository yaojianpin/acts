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
        let tasks = ctx.task.children();
        let mut complete_count = 0;
        for task in tasks.iter() {
            if task.state().is_pending() && task.is_ready() {
                let ctx = task.create_context(&ctx.scher);
                task.set_state(TaskState::Running);
                // state is changed, emit the task
                // because the state is the inner task state, not emit the message to client
                ctx.scher.emitter().emit_task_event_with_extra(task, false);
                task.exec(&ctx)?;
                return Ok(false);
            }

            if task.state().is_completed() {
                complete_count += 1;
            }
        }

        if complete_count == tasks.len() {
            ctx.task.set_action_state(ActionState::Completed);
            if let Some(next) = &ctx.task.node.next().upgrade() {
                ctx.sched_task(next);
                return Ok(false);
            }
            return Ok(true);
        }
        return Ok(false);
    }
}
