use crate::{
    ActTask, Result,
    model::Branch,
    scheduler::{Context, TaskState},
};
use async_trait::async_trait;
use tracing::debug;

#[async_trait]
impl ActTask for Branch {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        task.set_emit_disabled(true);
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

    fn next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        if task.state().is_running() {
            let children = task.node.children();
            if !children.is_empty() {
                for child in &children {
                    ctx.sched_task(child);
                }
            } else {
                task.set_state(TaskState::Completed);
            }
            return Ok(!children.is_empty());
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            task.set_state(TaskState::Completed);
            return Ok(true);
        } else if state.is_skip() {
            return Ok(true);
        }

        Ok(false)
    }
}
