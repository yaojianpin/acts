use crate::{
    event::ActionState,
    sch::{ActTask, TaskState},
    ActCatch, Context, Result,
};

impl ActTask for Vec<ActCatch> {
    fn error(&self, ctx: &Context) -> Result<()> {
        let err = ctx.err().unwrap_or_default();
        let mut is_catched = false;
        if self.len() > 0 {
            // process the specific err catch
            for c in self.iter().filter(|iter| iter.err.is_some()) {
                if let Some(code) = &c.err {
                    if code == &err.key {
                        ctx.sched_act(&c.id, &c.tag, &c.inputs, &c.outputs)?;
                        is_catched = true;
                        break;
                    }
                }
            }

            // process the any err catch
            if !is_catched {
                if let Some(c) = self.iter().find(|iter| iter.err.is_none()) {
                    ctx.sched_act(&c.id, &c.tag, &c.inputs, &c.outputs)?;
                    is_catched = true;
                }
            }
        }

        if !is_catched {
            ctx.task.set_pure_action_state(ActionState::Error);
            ctx.task.set_pure_state(TaskState::Fail(err.to_string()))
        }

        Ok(())
    }
}
