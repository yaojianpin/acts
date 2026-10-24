//! The `ReenterTask` operation: the durable rewind a `Cancel`/`Back` decision leaves
//! behind.
//!
//! The deciding commit closed the source generation and created this operation
//! in the *same* atomic batch, so the re-entry can be lost neither to a crash
//! nor to a lost compare-and-swap: its operation plans the fresh generations and
//! its durable ready record keeps it driven until that plan commits.

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
    async fn run(&self, _host: &KernelHost, _operation: &mut Operation<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        _host: &KernelHost,
        operation: &Operation<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let plan = {
            // generation and wrote the target it names (`Back` an earlier
            // step, `Cancel` the closed execution itself). Planning here —
            // not in the deciding call — is what makes the rewind survive
            // a lost compare-and-swap or a crash: its ready record keeps it
            // driven until its commit lands.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::{operation::ClaimToken, task_graph::TaskState};

    /// Without the durable target the rewind re-enters the source node itself:
    /// a `Cancel` closes the execution and names it as the target.
    #[tokio::test]
    async fn the_source_node_is_the_default_reenter_target() {
        let (host, _wake) = KernelHost::in_memory();
        let mut snapshot = ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap();
        let task_id = crate::scheduler::core::model::task_graph::TaskId::new("a", 0).unwrap();
        snapshot.tasks.get_mut(&task_id).unwrap().state = TaskState::Cancelled;
        let id = OperationId {
            process_id: "p".into(),
            task: task_id.clone(),
            sequence: 1,
        };
        let claim = OperationCommit {
            id: id.clone(),
            claim: ClaimToken {
                owner: "test".into(),
                fence: 1,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        };
        let mut lane = CommandLane::detached(&host);
        let operation = Operation {
            id: &id,
            claim: &claim,
            task_id: task_id.clone(),
            task: snapshot.tasks.get(&task_id).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let plan = ReenterTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap();
        assert!(
            plan.task_creates
                .iter()
                .any(|create| create.task.node.as_str() == "a" && create.task.generation == 1),
            "the rewind creates the next generation: {plan:?}"
        );
        assert!(
            plan.operations
                .iter()
                .any(|intent| intent.kind == OperationKind::ExecuteTask),
            "the re-entered root is admitted: {plan:?}"
        );
    }
}
