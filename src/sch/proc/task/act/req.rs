use crate::{
    sch::{Context, TaskState},
    ActTask, Irq, Result,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Irq {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if self.key.is_empty() {
            return Err(crate::ActError::Action(format!(
                "cannot find 'key' in act '{}'",
                task.node.id
            )));
        }
        task.set_state(TaskState::Interrupt);
        Ok(())
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            let tasks = task.children();

            let mut count = 0;
            for t in tasks.iter() {
                if t.state().is_error() {
                    ctx.emit_error()?;
                    return Ok(false);
                }
                if t.state().is_skip() {
                    task.set_state(TaskState::Skipped);
                    return Ok(true);
                }

                if t.state().is_success() {
                    count += 1;
                }
            }

            if count == tasks.len() && !task.state().is_completed() {
                task.set_state(TaskState::Completed);
            }
        }

        Ok(true)
    }
}
