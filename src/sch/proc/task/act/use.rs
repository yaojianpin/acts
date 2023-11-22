use crate::{
    sch::{ActTask, TaskState},
    utils::consts,
    ActUse, Context, Executor, Result,
};
use serde_json::json;

impl ActTask for ActUse {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.task.set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        if self.id.is_empty() {
            ctx.task.set_state(TaskState::Fail(format!(
                "not define id for act '{}'",
                ctx.task.id
            )));
            return self.error(ctx);
        }

        // start the workflow by id
        let executor = Executor::new(&ctx.scher);
        let mut inputs = ctx.task.inputs();
        inputs.insert(consts::PARENT_PROC_ID.to_string(), json!(ctx.proc.id()));
        inputs.insert(consts::PARENT_TASK_ID.to_string(), json!(ctx.task.id));
        executor.start(&self.id, &inputs)?;

        Ok(())
    }
}
