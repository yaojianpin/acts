use crate::{
    event::ActionState,
    utils::{self, consts},
    ActError, ActTask, Cmd, Context, Error, Result,
};

impl Cmd {
    pub fn run(&self, ctx: &Context) -> Result<()> {
        let set_inputs = || {
            if self.inputs.len() > 0 {
                let inputs = utils::fill_inputs(&ctx.task.env(), &self.inputs);
                ctx.task.env().set_env(&inputs);
            }
        };
        let name: &str = &self.name;
        match name {
            consts::EVT_SUBMIT => {
                set_inputs();
                ctx.task.set_action_state(ActionState::Submitted);
                ctx.task.next(ctx)?;
            }
            consts::EVT_COMPLETE => {
                set_inputs();
                ctx.task.set_action_state(ActionState::Completed);
                ctx.task.next(ctx)?;
            }
            consts::EVT_SKIP => {
                set_inputs();
                ctx.task.set_action_state(ActionState::Skipped);
                ctx.task.next(ctx)?;
            }
            consts::EVT_ABORT => {
                set_inputs();
                ctx.abort_task(&ctx.task)?;
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
                ctx.task.error(ctx)?;
            }
            _ => {
                return Err(ActError::Runtime(format!(
                    "cmd name not exists for '{name}'"
                )));
            }
        }
        Ok(())
    }
}
