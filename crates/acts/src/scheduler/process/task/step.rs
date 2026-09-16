use crate::{
    Act, ActTask, Result, Vars,
    model::Step,
    scheduler::{
        Context, NextAction, Sign, Task, TaskState,
        tree::{NodeContent, NodeOutputKind},
    },
    utils::consts,
};
use std::sync::Arc;

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

        // A timeout branch fires at most once per timed-out task. The marker is
        // claimed when the branch's guards hold — i.e. exactly when it is about
        // to run — and made durable before the branch's effect (its `run`
        // dispatches the act), so neither a later tick that re-dispatches the
        // branch nor a restored process can repeat it. A branch that already
        // fired skips instead, which keeps the chain fall-through reaching the
        // branches that have not fired.
        if let Some(owner) = task.timeout_owner() {
            let node_id = task.node().id();
            if !owner.claim_timeout(node_id) {
                task.set_state(TaskState::Skipped);
                return Ok(());
            }
            if let Err(err) = ctx.runtime.cache().upsert_async(&owner).await {
                owner.release_timeout(node_id);
                return Err(err);
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

        // A timeout branch belongs to the task that timed out, and only that
        // task's tick knows which of its branches is due — it is the only caller
        // that consults the one-shot markers. Advancing to the branch declared
        // after this one therefore goes through the same dispatcher the tick
        // uses, never through a raw `next`/`chain` jump: that path ignores the
        // markers, so a sibling that already fired (or is still in flight) would
        // gain a second task, once per tick.
        if let Some(owner) = task.timeout_owner() {
            let state = task.state();
            if !state.is_skip() && !state.is_biz_success() {
                return Ok(NextAction::Parent);
            }
            return if dispatch_next_timeout_branch(ctx, &owner)? {
                Ok(NextAction::Continue)
            } else {
                Ok(NextAction::Parent)
            };
        }

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

        if task.state().is_biz_success() {
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
        if task.node().children_in(NodeOutputKind::Timeout).is_empty() {
            return Ok(());
        }

        let cost = task.cost();
        // Write cost to parent task so timeout children read it via $cost()
        task.set_data_with(|data| data.set(consts::TASK_COST, cost));

        dispatch_next_timeout_branch(ctx, &task)?;

        Ok(())
    }
}

/// Dispatch the timed-out task's first branch that has neither fired nor is in
/// flight, and report whether one was dispatched.
///
/// This is the single dispatcher of a timed-out task's timeout branches: the
/// tick calls it, and a branch that just ran reaches the branch declared after
/// it through [`Step::next`] → here. A branch scheduling its declared sibling
/// directly would bypass the one-shot markers below, so a sibling that already
/// fired would gain a second task on every tick.
///
/// One branch per call, in declaration order: a branch whose guards do not hold
/// is skipped without claiming its marker, so a later call re-evaluates it (the
/// window may open), and the callers stop at the first branch that is still in
/// flight instead of running ahead of it.
fn dispatch_next_timeout_branch(ctx: &Context, owner: &Arc<Task>) -> Result<bool> {
    let NodeContent::Step(step) = &owner.node().content else {
        return Ok(false);
    };
    let tree = ctx.proc.tree();
    for branch in step.timeouts.iter() {
        if owner.is_timeout_claimed(&branch.id) {
            continue;
        }
        // `parent_id` (not `parent()`) keeps this scan lock-free: it runs while
        // `find_tasks` holds the process task lock.
        let in_flight = ctx.proc.find_tasks(|t| {
            t.node().id() == branch.id.as_str()
                && !t.state().is_completed()
                && t.parent_id().as_deref() == Some(owner.id.as_str())
        });
        if !in_flight.is_empty() {
            return Ok(false);
        }
        let Some(node) = tree.node(&branch.id) else {
            return Ok(false);
        };
        // `schedule_once`: an instance the tick (or a crash replay) already
        // created for this branch and predecessor is reused, not duplicated.
        ctx.schedule_once(&node, owner.clone())?;
        return Ok(true);
    }
    Ok(false)
}
