use crate::{
    sch::{ActTask, TaskState},
    utils::consts,
    Call, Context, Executor, Result,
};

impl ActTask for Call {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if self.mid.is_empty() {
            task.set_state(TaskState::Fail(format!(
                "cannot find 'mid' in act '{}'",
                task.id
            )));
            return self.error(ctx);
        }
        task.set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let executor = Executor::new(&ctx.scher);

        let mut inputs = task.inputs();
        inputs.set(consts::ACT_USE_PARENT_PROC_ID, &ctx.proc.id());
        inputs.set(consts::ACT_USE_PARENT_TASK_ID, &task.id);
        executor.start(&self.mid, &inputs)?;

        Ok(())
    }
}
