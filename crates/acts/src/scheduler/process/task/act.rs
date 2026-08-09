use crate::{
    Act, ActError, ActRunAs, ActTask, Result, TaskState, Vars,
    scheduler::{Context, NextAction},
    utils::consts,
};
use serde_json::Value as JsonValue;

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
                "cannot find 'uses' in act '{}'",
                task.node.id,
            )));
        }

        task.set_emit(false);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        task.set_emit(true);
        let package = ctx.runtime.store().packages().find(&self.uses)?;
        let in_scheam = serde_json::from_str::<JsonValue>(&package.schema)?;
        task.set_data_with(|data| data.set(consts::ACT_RUN_AS, package.run_as));
        match package.run_as {
            ActRunAs::Irq => {
                jsonschema::validate(&in_scheam, &task.params()).map_err(|err| {
                    ActError::Package(format!("package({}) validation error: {}", package.id, err))
                })?;
                // interrupt the state
                // irq act will complete by client action
                task.set_state(TaskState::Interrupt);
            }
            ActRunAs::Msg => {
                jsonschema::validate(&in_scheam, &task.params()).map_err(|err| {
                    ActError::Package(format!("package({}) validation error: {}", package.id, err))
                })?;
            }
            ActRunAs::Func => {
                let register = ctx
                    .runtime
                    .package()
                    .get(&package.id)
                    .ok_or(ActError::Runtime(format!(
                        "cannot find Func package '{}'",
                        package.id
                    )))?;
                let package = (register.create)(ctx.runtime.config())?;
                if let Some(vars) = package.execute(ctx, &ctx.task().params())? {
                    task.update_data(&vars);
                };
            }
        }

        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<NextAction> {
        let task = ctx.task();

        if task.state().is_interrupted() {
            return Ok(NextAction::Continue);
        } else if task.state().is_success() || task.state().is_skip() {
            // Schedule the next if the step.next is not empty
            if task.move_next(ctx)? {
                return Ok(NextAction::Continue);
            }
        }

        Ok(NextAction::Parent)
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
