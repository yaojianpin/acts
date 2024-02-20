use crate::{
    event::ActionState,
    utils::{self, consts},
    Act, ActTask, Catch, Context, Result, TaskState, Timeout,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum TaskLifeCycle {
    Created,
    Completed,
    Timeout,
    BeforeUpdate,
    Updated,
    Step,
    ErrorCatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatementBatch {
    Statement(Act),
    Catch(Catch),
    Timeout(Timeout),
}

impl StatementBatch {
    pub fn run(&self, ctx: &Context) -> Result<()> {
        match self {
            StatementBatch::Statement(s) => {
                s.exec(ctx)?;
            }
            StatementBatch::Catch(c) => {
                if let Some(err) = ctx.task.state().as_err() {
                    let is_catch_processed = ctx
                        .task
                        .env()
                        .get::<bool>(consts::IS_CATCH_PROCESSED)
                        .unwrap_or_default();
                    if is_catch_processed {
                        return Ok(());
                    }
                    // if the catch is no err key, it will catch all error
                    if c.err.is_none() || err.key == c.err {
                        ctx.task.env().set(consts::IS_CATCH_PROCESSED, true);
                        ctx.task.set_state(TaskState::Running);
                        ctx.task.set_pure_action_state(ActionState::Created);
                        for s in &c.then {
                            s.exec(ctx)?;
                        }

                        // review and run the next task
                        ctx.task.review(ctx)?;
                    }
                }
            }
            StatementBatch::Timeout(t) => {
                let key = format!("{}{}", consts::IS_TIMEOUT_PROCESSED_PREFIX, t.on);
                let is_timeout_processed = ctx.task.env().get::<bool>(&key).unwrap_or_default();
                if is_timeout_processed {
                    return Ok(());
                }

                let millis = utils::time::time() - ctx.task.start_time();
                if millis >= t.on.as_secs() * 1000 {
                    ctx.task.env().set(&key, true);
                    for s in &t.then {
                        s.exec(ctx)?;
                    }
                }
            }
        }

        Ok(())
    }
}
