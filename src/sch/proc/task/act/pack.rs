use crate::{sch::Context, ActError, ActTask, Pack, Result, StoreAdapter, TaskState};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Pack {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.task().set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let pack = ctx.runtime.cache().store().packages().find(&self.uses)?;
        let script: String = String::from_utf8(pack.file_data).map_err(ActError::from)?;
        ctx.eval(&script)?;

        if task.state().is_running() {
            task.set_state(TaskState::Completed);
        }
        Ok(())
    }
}
