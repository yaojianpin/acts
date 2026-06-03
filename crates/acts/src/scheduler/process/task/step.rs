use crate::{
    ActTask, Error, Result,
    model::Step,
    scheduler::{Context, TaskState},
    utils::consts,
};

impl ActTask for Step {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if let Some(expr) = &self.r#if {
            let cond = ctx.eval::<bool>(expr)?;
            if !cond {
                task.set_state(TaskState::Skipped);
                return Ok(());
            }
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let children = task.node.children();
        if !children.is_empty() {
            for child in &children {
                ctx.sched_task(child)?;
            }
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        let mut is_next: bool = false;
        if state.is_running() {
            let tasks = task.children();
            let mut count = 0;

            for task in tasks.iter() {
                if task.state().is_none() || task.state().is_running() {
                    is_next = true;
                } else if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.runtime.emitter().emit_task_event(task)?;
                    task.exec(ctx)?;
                    is_next = true;
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next)?;
                    return Ok(true);
                }
            }
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = task.node.next().upgrade() {
                ctx.sched_task(&next)?;
                return Ok(true);
            }
        }

        Ok(is_next)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            let tasks = task.children();
            let mut count = 0;
            for task in tasks.iter() {
                if task.state().is_pending() && task.is_ready() {
                    // resume task
                    task.set_state(TaskState::Running);
                    ctx.runtime.emitter().emit_task_event(task)?;
                    task.exec(ctx)?;
                    return Ok(false);
                }
                if task.state().is_completed() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !task.state().is_completed() {
                    // check if the task is error catched
                    let is_empty_catched = tasks
                        .iter()
                        .filter(|t| t.is_sign(consts::TASK_SIGN_CATCH))
                        .all(|t| t.state().is_skip());

                    if task.is_sign(consts::TASK_SIGN_ERR) && is_empty_catched {
                        // no any action to match
                        // resume the task error state
                        let err = task.with_data(|data| {
                            Error::new(
                                &data
                                    .get::<String>(consts::ACT_ERR_MESSAGE)
                                    .unwrap_or_default(),
                                &data.get::<String>(consts::ACT_ERR_CODE).unwrap_or_default(),
                            )
                        });
                        task.set_err(&err);
                        ctx.emit_error()?;
                        return Ok(false);
                    } else {
                        // if the task is originally error, clean the error data
                        // and set the task state to completed
                        if task.is_sign(consts::TASK_SIGN_ERR) {
                            task.set_data_with(|data| {
                                data.remove(consts::ACT_ERR_MESSAGE);
                                data.remove(consts::ACT_ERR_CODE);
                            });
                        }

                        task.set_state(TaskState::Completed);
                    }
                }

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next)?;
                    return Ok(false);
                }
                return Ok(true);
            }
        } else if state.is_skip() {
            // if the step is skipped, still find the next to run
            if let Some(next) = task.node.next().upgrade() {
                ctx.sched_task(&next)?;
                return Ok(false);
            }
            return Ok(true);
        }

        Ok(false)
    }
}
