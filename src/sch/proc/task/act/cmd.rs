use crate::{
    utils::{self, consts},
    ActError, ActTask, Do, Context, Error, Result, TaskState,
};

impl Do {
    pub fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if self.key.is_empty() {
            return Err(crate::ActError::Action(format!(
                "cannot find 'key' in act '{}'",
                task.node.id
            )));
        }
        let set_inputs = || {
            if self.inputs.len() > 0 {
                let inputs = utils::fill_inputs(&self.inputs, ctx);
                task.update_data(&inputs);
            }
        };
        let key: &str = &self.key;
        match key {
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
                let ecode =
                    self.inputs
                        .get::<String>(consts::ACT_ERR_CODE)
                        .ok_or(ActError::Action(format!(
                            "cannot find '{}' in cmd.inputs",
                            consts::ACT_ERR_CODE
                        )))?;
                let error = self
                    .inputs
                    .get::<String>(consts::ACT_ERR_MESSAGE)
                    .unwrap_or("".to_string());
                let err = Error::new(&error, &ecode);
                set_inputs();
                task.set_err(&err);
                task.error(ctx)?;
            }
            _ => {
                return Err(ActError::Runtime(format!(
                    "the cmd.name({key}) does not exists"
                )));
            }
        }
        Ok(())
    }
}
