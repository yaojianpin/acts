use tracing::debug;

use crate::{
    Act, ActError, ActRunAs, ActTask, Error, Result, TaskState, Vars, scheduler::Context,
    utils::consts,
};

impl ActTask for Act {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if let Some(expr) = &self.r#if {
            let cond = ctx.eval::<bool>(expr)?;
            if !cond {
                task.set_state(TaskState::Skipped);
                return Ok(());
            }
        }

        if self.uses.is_empty() {
            return Err(crate::ActError::Action(format!(
                "cannot find 'uses' in act '{}' with key '{}'",
                task.node.id,
                task.node.content.key()
            )));
        }

        let register = ctx
            .runtime
            .package()
            .get(&self.uses)
            .ok_or(ActError::Action(format!(
                "cannot find package '{}'",
                self.uses
            )))?;
        let meta = (register.meta)();
        match meta.run_as {
            ActRunAs::Irq => {
                jsonschema::validate(&meta.in_schema, &task.params())?;
                task.set_state(TaskState::Interrupt);
            }
            ActRunAs::Msg => {
                jsonschema::validate(&meta.in_schema, &task.params())?;
                task.set_emit_disabled(true);
                task.set_state(TaskState::Ready);
            }
            ActRunAs::Func => {
                task.set_emit_disabled(true);
                task.set_state(TaskState::Ready);
            }
        }

        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();

        let register = ctx
            .runtime
            .package()
            .get(&self.uses)
            .ok_or(ActError::Runtime(format!(
                "cannot find the registed package '{}'",
                self.uses
            )))?;
        let meta = (register.meta)();
        if matches!(meta.run_as, ActRunAs::Msg) {
            // resume the msg emit state
            task.set_emit_disabled(false);
        }

        if matches!(meta.run_as, ActRunAs::Func) {
            let package = (register.create)(ctx.task().params()).inspect_err(|err| {
                debug!("[{}] create error: {}", meta.id, err);
                task.set_err(&err.clone().into());
                ctx.set_task(&task);
                ctx.emit_error().ok();
            })?;

            if let Some(vars) = package.execute(ctx).inspect_err(|err| {
                debug!("[{}] execute error: {}", meta.id, err);
                task.set_err(&err.clone().into());
                ctx.set_task(&task);
                ctx.emit_error().ok();
            })? {
                task.update_data(&vars);
            };
        }

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
                if task.is_auto_complete() && !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next)?;
                    return Ok(true);
                }
            }
        } else if (state.is_skip() || state.is_success())
            && let Some(next) = &task.node.next().upgrade()
        {
            ctx.sched_task(next)?;
            return Ok(true);
        }
        Ok(is_next)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let task = ctx.task();
        let state = task.state();
        if state.is_running() {
            let tasks = task.children();
            let mut count = 0;
            for t in tasks.iter() {
                if t.state().is_error() {
                    ctx.emit_error()?;
                    return Ok(false);
                }
                // if t.state().is_skip() {
                //     task.set_state(TaskState::Skipped);
                //     return Ok(true);
                // }

                if t.state().is_success() || t.state().is_skip() {
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
                        task.set_state(TaskState::Completed);
                    }
                }

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next)?;
                    return Ok(false);
                }
                return Ok(true);
            }
        }

        Ok(false)
    }
}

impl Act {
    pub fn dispatch(&self, ctx: &Context, vars: Vars) -> Result<()> {
        let mut act = self.clone();
        if let Some(v) = ctx.get_var::<u32>(consts::ACT_INDEX) {
            act.vars.push(crate::Variant::create(consts::ACT_INDEX, v));
        }

        if let Some(v) = ctx.get_var::<String>(consts::ACT_VALUE) {
            act.vars.push(crate::Variant::create(consts::ACT_VALUE, v));
        }

        ctx.dispatch_act(self, vars)?;
        Ok(())
    }
}
