use crate::{sch::ActTask, utils::consts, Call, Context, Executor, Result};

impl ActTask for Call {
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
        let executor = Executor::new(&ctx.runtime);

        let mut inputs = task.inputs();
        inputs.set(consts::ACT_USE_PARENT_PROC_ID, &ctx.proc.id());
        inputs.set(consts::ACT_USE_PARENT_TASK_ID, &task.id);
        executor.proc().start(&self.key, &inputs)?;

        Ok(())
    }
}
