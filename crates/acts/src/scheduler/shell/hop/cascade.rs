//! The cascade hops: carrying a cancellation, an abort or a failure past an
//! execution that never ran its act.
//!
//! Both hops run nothing (phase 2 is the snapshot itself, `Outcome::Planned`);
//! phase 3 commits the execution's terminal state and hands the cascade on to
//! a `PropagateTask`, whose readiness treats the settled source as dead.

use super::*;
use crate::scheduler::shell::*;

/// The `SkipTask` hop: the cascade a dead predecessor carries past an
/// execution that was admitted but must not run.
pub(super) struct SkipTask;

#[async_trait::async_trait]
impl ActOperation for SkipTask {
    /// Nothing to check before the claim: a cascade hop always has its work.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole hop.
    async fn run(&self, _host: &KernelHost, _hop: &Hop<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        host: &KernelHost,
        hop: &Hop<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = hop.snapshot;
        let id = hop.id;
        let task_id = hop.task_id.clone();
        let claim = hop.claim;
        let task = hop.task.cloned();
        let plan = {
            // `Created → Ready`; this hop marks the execution `Skipped`
            // and hands propagation to a `PropagateTask`, whose readiness
            // treats the skipped source as dead and continues the
            // cascade. Like every hop it is one atomic commit sourced by
            // this operation.
            let task =
                task.ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
            if !matches!(
                task.state,
                crate::scheduler::core::model::task_graph::TaskState::Ready
                    | crate::scheduler::core::model::task_graph::TaskState::Running
                    | crate::scheduler::core::model::task_graph::TaskState::Waiting
            ) {
                return Err(ActError::Runtime(format!(
                    "execution {task_id:?} is {:?}, expected Ready, Running or Waiting",
                    task.state
                )));
            }
            let propagate_id = OperationId {
                process_id: id.process_id.clone(),
                node: id.node.clone(),
                generation: id.generation,
                sequence: id.sequence + 1,
            };
            let mut plan = host.task_hop_plan(
                snapshot,
                id,
                &task_id,
                Some(&task),
                claim,
                "skipped",
                "task.skipped",
            )?;
            // A skipped act emits its Skipped message (the legacy act
            // row's own message).
            if let Some(payload) = host.hop_message_payload(
                snapshot,
                &task_id,
                &id.node,
                act_facts(snapshot, &task_id).as_ref(),
                None,
                None,
            )? {
                plan.outbox[0].payload = payload.to_string();
            }
            plan.state_changes.push(StateChange {
                task: task_id.clone(),
                expected_revision: task.revision,
                from: task.state,
                to: crate::scheduler::core::model::task_graph::TaskState::Skipped,
            });
            if !snapshot.operations.contains_key(&propagate_id) {
                plan.operations.push(OperationIntent {
                    id: propagate_id,
                    kind: OperationKind::PropagateTask,
                });
            }
            plan
        };
        Ok(plan)
    }
}

/// The `AbortTask` hop: the abort cascade's running-successor half — the
/// legacy ancestor walk decides running children `Aborted` where a cancellation
/// decides them `Skipped`.
pub(super) struct AbortTask;

#[async_trait::async_trait]
impl ActOperation for AbortTask {
    /// Nothing to check before the claim: an abort hop always has its work.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole hop.
    async fn run(&self, _host: &KernelHost, _hop: &Hop<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        host: &KernelHost,
        hop: &Hop<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = hop.snapshot;
        let id = hop.id;
        let task_id = hop.task_id.clone();
        let claim = hop.claim;
        let task = hop.task.cloned();
        let plan = {
            // ancestor walk decides running children `Aborted` (pending
            // ones skip). Same shape as `SkipTask`, terminal `Aborted`.
            let task =
                task.ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
            if task.state != crate::scheduler::core::model::task_graph::TaskState::Running {
                // A decision landed while the hop was queued: the empty
                // remainder completes the operation either way.
                host.task_hop_plan(
                    snapshot,
                    id,
                    &task_id,
                    Some(&task),
                    claim,
                    "aborted",
                    "task.aborted",
                )?
            } else {
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    node: id.node.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let mut plan = host.task_hop_plan(
                    snapshot,
                    id,
                    &task_id,
                    Some(&task),
                    claim,
                    "aborted",
                    "task.aborted",
                )?;
                plan.state_changes.push(StateChange {
                    task: task_id.clone(),
                    expected_revision: task.revision,
                    from: crate::scheduler::core::model::task_graph::TaskState::Running,
                    to: crate::scheduler::core::model::task_graph::TaskState::Aborted,
                });
                // The aborted execution announces itself (the legacy
                // `on_task` emitted its Aborted message like any other).
                if let Some(payload) = host.hop_message_payload(
                    snapshot,
                    &task_id,
                    &id.node,
                    act_facts(snapshot, &task_id).as_ref(),
                    None,
                    None,
                )? {
                    plan.outbox[0].payload = payload.to_string();
                }
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                    });
                }
                plan
            }
        };
        Ok(plan)
    }
}
