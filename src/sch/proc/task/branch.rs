use crate::{
    event::ActionState,
    model::Branch,
    sch::{Context, TaskState},
    ActTask, Result,
};
use async_trait::async_trait;
use tracing::debug;

#[async_trait]
impl ActTask for Branch {
    fn init(&self, ctx: &Context) -> Result<()> {
        if self.needs.len() > 0 {
            ctx.task.set_state(TaskState::Pending);
            return Ok(());
        }

        match &self.r#if {
            Some(expr) => {
                let is_true = ctx.eval(expr)?;
                debug!("{} = {}", expr, is_true);
                if !is_true {
                    ctx.task.set_action_state(ActionState::Skipped);
                    return Ok(());
                }
            }
            None => {
                let mut branch_count = 1;
                if let Some(parent) = ctx.task.node.parent() {
                    branch_count = parent.children().len();
                }

                if !self.r#else {
                    ctx.task.set_action_state(ActionState::Skipped);
                    return Ok(());
                }

                if branch_count > 1 {
                    ctx.task.set_state(TaskState::Pending);
                }

                return Ok(());
            }
        };

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if let Some(script) = &self.run {
            ctx.run(script)?;
        }
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        if ctx.task.state().is_running() {
            let children = ctx.task.node.children();
            if children.len() > 0 {
                for child in &ctx.task.node.children() {
                    ctx.sched_task(child);
                }
            } else {
                ctx.task.set_action_state(ActionState::Completed);
            }
            return Ok(children.len() > 0);
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let state = ctx.task.state();
        if state.is_running() {
            ctx.task.set_action_state(ActionState::Completed);
            return Ok(true);
        } else if state.is_skip() {
            return Ok(true);
        }

        Ok(false)
    }
}
