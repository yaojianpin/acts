use crate::{ActTask, Result, TaskState, Workflow, scheduler::Context};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Workflow {
    fn init(&self, ctx: &Context) -> Result<()> {
        // set the env to process env local
        if !self.env.is_empty() {
            ctx.proc.with_env_local_mut(|data| {
                for (k, v) in self.env.iter() {
                    data.set(k, v.clone());
                }
            });
        }

        // run setup
        if !self.setup.is_empty() {
            ctx.dispatch_acts(self.setup.clone(), true)?;
            // for s in &self.setup {
            //     s.exec(ctx)?;
            // }
        }
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let children = task.node.children();
        if !children.is_empty() {
            for step in &children {
                ctx.sched_task(step);
            }
        } else {
            task.set_state(TaskState::Completed);
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let tasks = task.children();

        Ok(tasks.iter().all(|t| t.state().is_completed()))
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            task.set_state(TaskState::Completed);
            return Ok(true);
        }

        Ok(false)
    }
}
