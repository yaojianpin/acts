use super::hook::TaskLifeCycle;
use crate::{
    model::Step,
    scheduler::{Context, NodeContent, TaskState},
    ActError, ActTask, Result, StoreAdapter,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Step {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if let Some(expr) = &self.r#if {
            let cond = ctx.eval::<bool>(expr)?;
            if !cond {
                task.set_state(TaskState::Skipped);
                return Ok(());
            }
        }

        // add catch hooks
        if !self.catches.is_empty() {
            for c in &self.catches {
                task.add_hook_catch(TaskLifeCycle::ErrorCatch, c);
            }
        }

        // add timeout hooks
        if !self.timeout.is_empty() {
            for s in &self.timeout {
                task.add_hook_timeout(TaskLifeCycle::Timeout, s);
            }
        }

        // run setup
        if !self.setup.is_empty() {
            for act in &self.setup {
                act.exec(ctx)?;
            }
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if let Some(script) = &self.run {
            ctx.eval::<()>(script)?;
        }

        if let Some(pack_id) = &self.uses {
            let pack = ctx.runtime.cache().store().packages().find(pack_id)?;
            let script: String = String::from_utf8(pack.data).map_err(ActError::from)?;
            ctx.eval::<()>(&script)?;
        }
        let children = task.node.children();
        if !children.is_empty() {
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

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next);
                    return Ok(true);
                }
            }
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = task.node.next().upgrade() {
                ctx.sched_task(&next);
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
                if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.runtime.scher().emit_task_event(task)?;
                    task.exec(ctx)?;
                    return Ok(false);
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next);
                    return Ok(false);
                }
                return Ok(true);
            }
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = task.node.next().upgrade() {
                ctx.sched_task(&next);
                return Ok(false);
            }
            return Ok(true);
        }

        Ok(false)
    }
}
