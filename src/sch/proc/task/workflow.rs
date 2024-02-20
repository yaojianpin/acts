use crate::{event::ActionState, sch::Context, ActTask, Result, Workflow};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Workflow {
    fn init(&self, ctx: &Context) -> Result<()> {
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
        let children = ctx.task.node.children();
        if children.len() > 0 {
            for step in &children {
                ctx.sched_task(step);
            }
        } else {
            ctx.task.set_action_state(ActionState::Completed);
        }

        Ok(children.len() > 0)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        if state.is_running() {
            ctx.task.set_action_state(ActionState::Completed);
            return Ok(true);
        }

        Ok(false)
    }
}
