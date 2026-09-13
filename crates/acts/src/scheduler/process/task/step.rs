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
        if let Some(owner) = timeout_owner(&task) {
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
        if task.node().children_in(NodeOutputKind::Timeout).is_empty() {
            return Ok(());
        }

        let cost = task.cost();
        // Write cost to parent task so timeout children read it via $cost()
        task.set_data_with(|data| data.set(consts::TASK_COST, cost));

        // The branches are dispatched one at a time, in declaration order: a
        // tick schedules the first branch that has not fired, and that branch's
        // `next`/`chain` fall-through evaluates the ones after it — so a branch
        // whose window has not opened is skipped, not fired. The marker is
        // claimed by the branch itself when its guards hold (see `init`), which
        // is what keeps a false-guarded branch's slot open for a later tick.
        //
        // A branch that is still queued or running is never dispatched a
        // second time: the marker only records the branches that already fired,
        // and a tick landing while the previous dispatch is still in flight
        // must not create a duplicate. Once every branch has fired there is
        // nothing left to reach — dispatching again would only create one task
        // per tick.
        let tree = ctx.proc.tree();
        for branch in self.timeouts.iter() {
            if task.is_timeout_claimed(&branch.id) {
                continue;
            }
            // `parent_id` (not `parent()`) keeps this scan lock-free: it runs
            // while `find_tasks` holds the process task lock.
            let in_flight = ctx.proc.find_tasks(|t| {
                t.node().id() == branch.id.as_str()
                    && !t.state().is_completed()
                    && t.parent_id().as_deref() == Some(task.id.as_str())
            });
            if !in_flight.is_empty() {
                break;
            }
            if let Some(node) = tree.node(&branch.id) {
                ctx.sched_task(&node, task.clone())?;
            }
            break;
        }

        Ok(())
    }
}

/// The timed-out task a timeout branch belongs to, when `task`'s node is
/// declared as one of its step's `timeouts`. That task owns the one-shot
/// marker of the branch (see [`Task::claim_timeout`]); the branch is a partial
/// projection of its declaration, so the declaration is the identity.
fn timeout_owner(task: &Arc<Task>) -> Option<Arc<Task>> {
    let parent = task.parent()?;
    match &parent.node().content {
        NodeContent::Step(step)
            if step
                .timeouts
                .iter()
                .any(|branch| branch.id == task.node().id()) =>
        {
            Some(parent)
        }
        _ => None,
    }
}
