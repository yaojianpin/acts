use super::hook::TaskLifeCycle;
use crate::{
    event::ActionState,
    model::Step,
    sch::{Context, NodeContent, TaskState},
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

        // add catch hooks
        if self.catches.len() > 0 {
            for c in &self.catches {
                ctx.task.add_hook_catch(TaskLifeCycle::ErrorCatch, c);
            }
        }

        // add timeout hooks
        if self.timeout.len() > 0 {
            for s in &self.timeout {
                ctx.task.add_hook_timeout(TaskLifeCycle::Timeout, s);
            }
        }

        // run setup
        if self.setup.len() > 0 {
            for s in &self.setup {
                s.exec(ctx)?;
            }
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if let Some(script) = &self.run {
            ctx.run(script)?;
        }

        let children = ctx.task.node.children();
        if children.len() > 0 {
            for child in &children {
                if let NodeContent::Act(act) = &child.content {
                    if act.is_taskable() {
                        ctx.sched_task(child);
                    } else {
                        act.exec(ctx)?;
                    }
                } else {
                    ctx.sched_task(child);
                }
            }
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
                // else if task.state().is_error() {
                //     ctx.set_err(&task.state().as_err().unwrap_or_default());
                //     ctx.emit_error()?;
                //     return Ok(false);
                // }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !ctx.task.state().is_completed() {
                    ctx.task.set_action_state(ActionState::Completed);
                }

                if let Some(next) = &ctx.task.node.next().upgrade() {
                    ctx.sched_task(next);
                    return Ok(true);
                }
            }
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = ctx.task.node.next().upgrade() {
                ctx.sched_task(&next);
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
                if task.state().is_pending() && task.is_ready() {
                    let ctx = task.create_context(&ctx.scher);

                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.scher.emitter().emit_task_event(task);
                    task.exec(&ctx)?;
                    return Ok(false);
                }
                // else if task.state().is_error() {
                //     ctx.set_err(&task.state().as_err().unwrap_or_default());
                //     ctx.emit_error()?;
                //     return Ok(false);
                // }

                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
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

    // fn error(&self, ctx: &Context) -> Result<()> {
    //     self.catches.run(ctx)?;
    //     if ctx.task.state().is_error() {
    //         return ctx.emit_error();
    //     }
    //     Ok(())
    // }
}
