use crate::{
    sch::{ActTask, TaskState},
    utils::consts,
    Context, Executor, Result, Use,
};

impl ActTask for Use {
    fn init(&self, ctx: &Context) -> Result<()> {
        if self.mid.is_empty() {
            ctx.task.set_state(TaskState::Fail(format!(
                "not find 'mid' in act '{}'",
                ctx.task.id
            )));
            return self.error(ctx);
        }
        ctx.task.set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let executor = Executor::new(&ctx.scher);

        let mut inputs = ctx.task.inputs();
        inputs.set(consts::ACT_USE_PARENT_PROC_ID, &ctx.proc.id());
        inputs.set(consts::ACT_USE_PARENT_TASK_ID, &ctx.task.id);
        executor.start(&self.mid, &inputs)?;

        Ok(())
    }
}
