use crate::{
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
                let task = ctx.task();
                if let Some(err) = task.err() {
                    let is_catch_processed = task
                        .with_data(|data| data.get::<bool>(consts::IS_CATCH_PROCESSED))
                        .unwrap_or_default();
                    if is_catch_processed {
                        return Ok(());
                    }
                    // if the catch is no err key, it will catch all error
                    if c.err.is_none() || &err.ecode == c.err.as_ref().unwrap() {
                        task.set_data_with(|data| data.set(consts::IS_CATCH_PROCESSED, true));
                        task.set_state(TaskState::Running);
                        for s in &c.then {
                            s.exec(ctx)?;
                        }

                        // review and run the next task
                        task.review(ctx)?;
                    }
                }
            }
            StatementBatch::Timeout(t) => {
                let task = ctx.task();
                let key = format!("{}{}", consts::IS_TIMEOUT_PROCESSED_PREFIX, t.on);
                let is_timeout_processed = task
                    .with_data(|data| data.get::<bool>(&key))
                    .unwrap_or_default();
                if is_timeout_processed {
                    return Ok(());
                }

                let millis = utils::time::time() - task.start_time();
                if millis >= t.on.as_secs() * 1000 {
                    task.set_data_with(|data| data.set(&key, true));
                    for s in &t.then {
                        s.exec(ctx)?;
                    }
                }
            }
        }

        Ok(())
    }
}
