use crate::{scheduler::Context, ActError, ActTask, Pack, Result, StoreAdapter, TaskState};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Pack {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if self.key.is_empty() {
            return Err(crate::ActError::Action(format!(
                "cannot find 'key' in act '{}'",
                task.node.id
            )));
        }
        task.set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let pack = ctx.runtime.cache().store().packages().find(&self.key)?;
        let script: String = String::from_utf8(pack.data).map_err(ActError::from)?;
        ctx.eval::<()>(&script)?;

        if task.state().is_running() {
            task.set_state(TaskState::Completed);
        }
        Ok(())
    }
}
