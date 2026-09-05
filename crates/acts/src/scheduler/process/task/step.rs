use crate::{
    Act, ActTask, Result, Vars,
    model::Step,
    scheduler::{Context, NextAction, Sign, TaskState, tree::NodeOutputKind},
    utils::consts,
};

impl ActTask for Step {
    async fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if let Some(expr) = &self.r#if {
            let cond = ctx.eval::<bool>(expr)?;
            if !cond {
                task.set_state(TaskState::Skipped);
                return Ok(());
            }
        }

        // the while condition is the loop gate: it is re-evaluated at the
        // start of every iteration, and a false value exits the loop by
        // skipping this step (falling through to the chain)
        if let Some(expr) = &self.r#while {
            let cond = ctx.eval::<bool>(expr)?;
            if !cond {
                task.set_state(TaskState::Skipped);
                return Ok(());
            }
        }
        Ok(())
    }

    async fn run(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        if let Some(uses) = &self.uses {
            ctx.dispatch_act(
                &Act {
                    name: self.name.clone(),
                    uses: uses.to_string(),
                    params: task.params(),
                    options: self.options.clone(),
                    ..Default::default()
                },
                self.vars(),
            )?;
        }
        Ok(())
    }

    async fn next(&self, ctx: &Context) -> Result<NextAction> {
        let task = ctx.task();

        if task.state().is_skip() {
            // A skipped step (its `if`/`while` condition failed) must not
            // take its explicit `next` jump — following a self/backward
            // `next` would re-enter the loop forever. Fall through to the
            // step declared after it; without one the flow moves to the parent.
            if let Some(chain) = task.node().chain().upgrade() {
                ctx.schedule_once(&chain, ctx.task())?;
                return Ok(NextAction::Continue);
            }
            return Ok(NextAction::Parent);
        }

        if task.state().is_success() {
            if self.r#while.is_some() {
                // loop back to self; the next iteration re-evaluates the
                // while condition in init and exits (skips) when it fails
                ctx.schedule_once(task.node(), ctx.task())?;
                return Ok(NextAction::Continue);
            }
            // Schedule the next if the step.next is not empty
            if task.move_next(ctx)? {
                return Ok(NextAction::Continue);
            }
        }

        Ok(NextAction::Parent)
    }

    async fn on_error(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let children = task.node().children_in(NodeOutputKind::Catch);
        if task.sign().is_none() && !children.is_empty() {
            task.set_sign(Sign::ERROR);
            task.set_state(TaskState::Running);
            for child in &children {
                ctx.sched_task_with_vars(
                    child,
                    Vars::new().with(consts::TASK_SIGN, Sign::CATCH),
                    task.clone(),
                )?;
            }
        }
        ctx.emit_error().await
    }

    async fn on_timeout(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        let children = task.node().children_in(NodeOutputKind::Timeout);
        if !children.is_empty() {
            let cost = task.cost();
            // Write cost to parent task so timeout children read it via $cost()
            task.set_data_with(|data| data.set(consts::TASK_COST, cost));
            for child in &children {
                ctx.sched_task(child, task.clone())?;
            }
        }

        Ok(())
    }
}
