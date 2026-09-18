use crate::{
    ActTask, Result, TaskState, Workflow,
    scheduler::{Context, NextAction},
};

impl ActTask for Workflow {
    async fn init(&self, ctx: &Context) -> Result<()> {
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

    async fn next(&self, ctx: &Context) -> Result<NextAction> {
        let task = ctx.task();

        if task.children().iter().all(|t| t.state().is_completed()) {
            // only complete a workflow that is still running: a client abort
            // that landed while the children settled must stick
            task.set_state_if_running(TaskState::Completed);
        }
        Ok(NextAction::Parent)
    }
}
