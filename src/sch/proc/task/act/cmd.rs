use crate::{
    utils::{self, consts},
    ActError, ActTask, Cmd, Context, Error, Result, TaskState, Vars,
};

impl Cmd {
    pub fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let set_inputs = || {
            if self.inputs.len() > 0 {
                let inputs = utils::fill_inputs(&self.inputs, ctx);
                task.update_data(&inputs);
            }
        };
        let name: &str = &self.name;
        match name {
            consts::EVT_SUBMIT => {
                set_inputs();
                task.set_state(TaskState::Submitted);
                task.next(ctx)?;
            }
            consts::EVT_NEXT => {
                set_inputs();
                task.set_state(TaskState::Completed);
                task.next(ctx)?;
            }
            consts::EVT_SKIP => {
                set_inputs();
                task.set_state(TaskState::Skipped);
                task.next(ctx)?;
            }
            consts::EVT_ABORT => {
                set_inputs();
                ctx.abort_task(&task)?;
            }
            consts::EVT_ERR => {
                let err = self
                    .inputs
                    .get::<Vars>(consts::ACT_ERR_KEY)
                    .ok_or(ActError::Action(format!(
                        "cannot find '{}' in cmd.inputs",
                        consts::ACT_ERR_KEY
                    )))?;

                let err = Error::from_var(&err)?;
                set_inputs();
                task.set_err(&err);
                task.error(ctx)?;
            }
            _ => {
                return Err(ActError::Runtime(format!(
                    "the cmd.name({name}) does not exists"
                )));
            }
        }
        Ok(())
    }
}
