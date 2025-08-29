use super::TaskLifeCycle;
use crate::{
    Act, ActError, ActRunAs, ActTask, Result, TaskState, scheduler::Context, utils::consts,
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

        for s in self.catches.iter() {
            task.add_hook_catch(TaskLifeCycle::ErrorCatch, s);
        }

        if !self.timeout.is_empty() {
            for s in &self.timeout {
                task.add_hook_timeout(TaskLifeCycle::Timeout, s);
            }
        }

        // run setup
        if !self.setup.is_empty() {
            ctx.dispatch_acts(self.setup.clone(), true)?;
        }

        if self.uses.is_empty() {
            return Err(crate::ActError::Action(format!(
                "cannot find 'uses' in act '{}' with key '{}'",
                task.node.id,
                task.node.content.key()
            )));
        }

        // find the package to run
        let package = ctx.executor.pack().get(&self.uses)?;
        let schema: serde_json::Value = serde_json::from_str(&package.schema)?;
        match package.run_as {
            ActRunAs::Irq => {
                jsonschema::validate(&schema, &task.params())?;
                task.set_state(TaskState::Interrupt);
            }
            ActRunAs::Msg => {
                jsonschema::validate(&schema, &task.params())?;
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

        // find the package to run
        let package = ctx.executor.pack().get(&self.uses)?;
        if matches!(package.run_as, ActRunAs::Msg) {
            // resume the msg emit state
            task.set_emit_disabled(false);
        }

        if matches!(package.run_as, ActRunAs::Func) {
            let register = ctx
                .runtime
                .package()
                .get(&self.uses)
                .ok_or(ActError::Runtime(format!(
                    "cannot find the registed package '{}'",
                    self.uses
                )))?;
            let package = (register.create)(ctx.task().params())?;
            if let Some(vars) = package.execute(ctx)? {
                task.update_data(&vars);
            }
        }

        let children = task.node.children();
        if !children.is_empty() {
            for child in &children {
                ctx.sched_task(child);
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
                    ctx.runtime.scher().emit_task_event(task)?;

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
                    ctx.sched_task(next);
                    return Ok(true);
                }
            }
        } else if (state.is_skip() || state.is_success())
            && let Some(next) = &task.node.next().upgrade()
        {
            ctx.sched_task(next);
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
                if t.state().is_skip() {
                    task.set_state(TaskState::Skipped);
                    return Ok(true);
                }

                if t.state().is_success() {
                    count += 1;
                }
            }

            if count == tasks.len() {
                if !task.state().is_completed() {
                    task.set_state(TaskState::Completed);
                }

                if let Some(next) = &task.node.next().upgrade() {
                    ctx.sched_task(next);
                    return Ok(false);
                }
                return Ok(true);
            }
        }

        Ok(false)
    }
}

impl Act {
    pub fn dispatch(&self, ctx: &Context, is_hook_event: bool) -> Result<()> {
        // let package = ctx.executor.pack().get(&self.uses)?;
        let mut act = self.clone();
        if let Some(v) = ctx.get_var::<u32>(consts::ACT_INDEX) {
            act.inputs.set(consts::ACT_INDEX, v);
        }

        if let Some(v) = ctx.get_var::<String>(consts::ACT_VALUE) {
            act.inputs.set(consts::ACT_VALUE, v);
        }

        ctx.dispatch_act(self, is_hook_event)?;
        Ok(())
    }
}
