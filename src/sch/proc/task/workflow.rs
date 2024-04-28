use crate::{sch::Context, ActTask, Result, TaskState, Workflow};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Workflow {
    fn init(&self, ctx: &Context) -> Result<()> {
        // set the env to proc env local
        if self.env.len() > 0 {
            ctx.proc.with_env_local_mut(|data| {
                for (k, v) in self.env.iter() {
                    data.set(k, v.clone());
                }
            });
        }

        // run setup
        if self.setup.len() > 0 {
            for s in &self.setup {
                s.exec(ctx)?;
            }
        }

        Ok(())
    }

    fn run(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let children = task.node.children();
        if children.len() > 0 {
            for step in &children {
                ctx.sched_task(step);
            }
        } else {
            task.set_state(TaskState::Completed);
        }

        Ok(children.len() > 0)
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
