use crate::{
    event::ActionState,
    utils::{self, consts},
    ActError, ActTask, Cmd, Context, Error, Result,
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
                task.set_action_state(ActionState::Submitted);
                task.next(ctx)?;
            }
            consts::EVT_COMPLETE => {
                set_inputs();
                task.set_action_state(ActionState::Completed);
                task.next(ctx)?;
            }
            consts::EVT_SKIP => {
                set_inputs();
                task.set_action_state(ActionState::Skipped);
                task.next(ctx)?;
            }
            consts::EVT_ABORT => {
                set_inputs();
                ctx.abort_task(&task)?;
            }
            consts::EVT_ERR => {
                let err_code =
                    self.inputs
                        .get::<String>(consts::ACT_ERR_CODE)
                        .ok_or(ActError::Action(format!(
                            "cannot find '{}' in cmd.inputs",
                            consts::ACT_ERR_CODE
                        )))?;

                if err_code.is_empty() {
                    return Err(ActError::Action(format!(
                        "the var '{}' cannot be empty in cmd.inputs",
                        consts::ACT_ERR_CODE
                    )));
                }
                let err_message = self
                    .inputs
                    .get::<String>(consts::ACT_ERR_MESSAGE)
                    .unwrap_or_default();
                ctx.set_err(&Error {
                    key: Some(err_code.to_string()),
                    message: err_message,
                });

                set_inputs();
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
