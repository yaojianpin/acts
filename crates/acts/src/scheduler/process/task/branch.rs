use crate::{
    ActTask, Result,
    model::Branch,
    scheduler::{Context, TaskState},
};
use tracing::debug;

impl ActTask for Branch {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        task.set_emit(false);
        if !self.needs.is_empty() {
            task.set_state(TaskState::Pending);
            return Ok(());
        }

        match &self.r#if {
            Some(expr) => {
                let is_true = ctx.eval::<bool>(expr)?;
                debug!("{} = {}", expr, is_true);
                if !is_true {
                    task.set_state(TaskState::Skipped);
                    return Ok(());
                }
            }
            None => {
                let mut branch_count = 1;
                if let Some(parent) = task.node.parent() {
                    branch_count = parent.children().len();
                }

                if !self.r#else {
                    task.set_state(TaskState::Skipped);
                    return Ok(());
                }

                if branch_count > 1 {
                    task.set_state(TaskState::Pending);
                }

                return Ok(());
            }
        };

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if let Some(script) = &self.run {
            ctx.eval::<()>(script)?;
        }
        Ok(())
    }
}
