//! The `ReenterTask` operation: the durable rewind a `Cancel`/`Back` decision leaves
//! behind.
//!
//! The deciding commit closed the source generation and created this operation
//! in the *same* atomic batch, so the re-entry can be lost neither to a crash
//! nor to a lost compare-and-swap: its operation plans the fresh generations and the
//! recovery scan re-drives it until that plan commits.

use super::*;
use crate::scheduler::shell::*;

/// The rewind operation: `Back` to an earlier step, or `Cancel` of the closed
/// execution itself.
pub(super) struct ReenterTask;

#[async_trait::async_trait]
impl ActOperation for ReenterTask {
    /// Nothing to check before the claim: a rewind always has its work.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole operation.
    async fn run(&self, _host: &KernelHost, _operation: &Operation<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        _host: &KernelHost,
        operation: &Operation<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = operation.snapshot;
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let plan = {
            // generation and wrote the target it names (`Back` an earlier
            // step, `Cancel` the closed execution itself). Planning here —
            // not in the deciding call — is what makes the rewind survive
            // a lost compare-and-swap or a crash: this operation is re-driven by
            // the recovery scan until its commit lands.
            let target = snapshot
                .variables
                .get(&(task_id.clone(), REENTER_TARGET_VAR.to_string()))
                .cloned()
                .unwrap_or_else(|| id.task.node.as_str().to_owned());
            let graph = task_graph(snapshot)?;
            let command = crate::scheduler::core::plan::generation::ReenterCommand {
                process_id: id.process_id.clone(),
                source: task_id.clone(),
                target_task_id: target,
                source_operation: claim.clone(),
            };
            Reenter::plan((&graph, snapshot), &command).map_err(runtime)?
        };
        Ok(plan)
    }
}
