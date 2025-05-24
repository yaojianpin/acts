use crate::{
    Act, ActTask, Catch, Context, Result, TaskState, Timeout,
    model::TimeoutLimit,
    scheduler::tree::NodeOutputKind,
    utils::{self, consts},
};
use serde::{Deserialize, Serialize};
use tracing::debug;

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
                s.dispatch(ctx, true)?;
            }
            StatementBatch::Catch(c) => {
                let task = ctx.task();
                debug!("run catch: {:?}", c);
                if let Some(err) = task.err() {
                    let is_catch_processed = task
                        .with_data(|data| data.get::<bool>(consts::IS_CATCH_PROCESSED))
                        .unwrap_or_default();
                    if is_catch_processed {
                        return Ok(());
                    }

                    // if the catch is no err key, it will catch all error
                    if c.on.is_none() || &err.ecode == c.on.as_ref().unwrap() {
                        task.set_data_with(|data| data.set(consts::IS_CATCH_PROCESSED, true));
                        task.set_state(TaskState::Running);

                        let children = task.node().children_in(NodeOutputKind::Catch, c.on.clone());

                        if !children.is_empty() {
                            for node in
                                &task.node().children_in(NodeOutputKind::Catch, c.on.clone())
                            {
                                ctx.sched_task(node);
                            }
                        } else {
                            // review and run the next task
                            // no catched children means to ignore the error
                            task.review(ctx)?;
                        }
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

                let millis = utils::time::time_millis() - task.start_time();
                let on = TimeoutLimit::parse(&t.on)?;
                if millis >= on.as_secs() * 1000 {
                    task.set_data_with(|data| data.set(&key, true));
                    for node in &task
                        .node()
                        .children_in(NodeOutputKind::Timeout, Some(t.on.clone()))
                    {
                        ctx.sched_task(node);
                    }
                }
            }
        }

        Ok(())
    }
}
