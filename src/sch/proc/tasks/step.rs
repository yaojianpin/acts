use crate::{
    model::Step,
    sch::{proc::utils::dispatcher::Dispatcher, Context, TaskState},
    ActTask,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Step {
    fn prepare(&self, _ctx: &Context) {}

    fn run(&self, ctx: &Context) {
        let dispatcher = Dispatcher::new(ctx);
        if let Some(expr) = &self.r#if {
            match ctx.eval(expr) {
                Ok(cond) => {
                    if cond {
                        let ret = dispatcher.process(self);
                        if ret.is_err() {
                            ctx.task
                                .set_state(TaskState::Fail(ret.err().unwrap().into()))
                        }
                    } else {
                        ctx.task.set_state(TaskState::Skip);
                    }
                }
                Err(err) => ctx.task.set_state(TaskState::Fail(err.into())),
            }
        } else {
            let ret = dispatcher.process(self);
            if ret.is_err() {
                ctx.task
                    .set_state(TaskState::Fail(ret.err().unwrap().into()))
            }
        }
    }

    fn post(&self, ctx: &Context) {
        ctx.task.set_state(TaskState::Success);
    }
}
