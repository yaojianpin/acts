use crate::{
    ActTask, Result, TaskState, Workflow,
    scheduler::{Context, NextAction},
};

impl ActTask for Workflow {
    fn init(&self, ctx: &Context) -> Result<()> {
        // init process env
        if !self.env.is_empty() {
            ctx.proc.with_env_mut(|data| {
                for var in self.env.iter() {
                    data.set(&var.name, var.value.clone());
                }
            });
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<NextAction> {
        let task = ctx.task();

        if task.children().iter().all(|t| t.state().is_completed()) && task.state().is_running() {
            task.set_state(TaskState::Completed);
        }
        Ok(NextAction::Parent)
    }
}
