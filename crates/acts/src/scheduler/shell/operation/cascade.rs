//! The cascade operations: carrying a cancellation, an abort or a failure past an
//! execution that never ran its act.
//!
//! Both operations run nothing (phase 2 is the snapshot itself, `Outcome::Planned`);
//! phase 3 commits the execution's terminal state and hands the cascade on to
//! a `PropagateTask`, whose readiness treats the settled source as dead.

use super::*;
use crate::scheduler::shell::*;

/// The `SkipTask` operation: the cascade a dead predecessor carries past an
/// execution that was admitted but must not run.
pub(super) struct SkipTask;

#[async_trait::async_trait]
impl ActOperation for SkipTask {
    /// Nothing to check before the claim: a cascade operation always has its work.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole operation.
    async fn run(&self, _host: &KernelHost, _operation: &mut Operation<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let task = operation.task().cloned();
        let plan = {
            // `Created → Ready`; this operation marks the execution `Skipped`
            // and hands propagation to a `PropagateTask`, whose readiness
            // treats the skipped source as dead and continues the
            // cascade. Like every operation it is one atomic commit sourced by
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
                task: id.task.clone(),
                sequence: id.sequence + 1,
            };
            let mut plan = host.task_operation_plan(
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
            if let Some(payload) = host.operation_message_payload(
                snapshot,
                &task_id,
                id.task.node.as_str(),
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
                    ready_at_ms: None,
                });
            }
            plan
        };
        Ok(plan)
    }
}

/// The `AbortTask` operation: the abort cascade's running-successor half — the
/// legacy ancestor walk decides running children `Aborted` where a cancellation
/// decides them `Skipped`.
pub(super) struct AbortTask;

#[async_trait::async_trait]
impl ActOperation for AbortTask {
    /// Nothing to check before the claim: an abort operation always has its work.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole operation.
    async fn run(&self, _host: &KernelHost, _operation: &mut Operation<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let task = operation.task().cloned();
        let plan = {
            // ancestor walk decides running children `Aborted` (pending
            // ones skip). Same shape as `SkipTask`, terminal `Aborted`.
            let task =
                task.ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
            if task.state != crate::scheduler::core::model::task_graph::TaskState::Running {
                // A decision landed while the operation was queued: the empty
                // remainder completes the operation either way.
                host.task_operation_plan(
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
                    task: id.task.clone(),
                    sequence: id.sequence + 1,
                };
                let mut plan = host.task_operation_plan(
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
                if let Some(payload) = host.operation_message_payload(
                    snapshot,
                    &task_id,
                    id.task.node.as_str(),
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
                        ready_at_ms: None,
                    });
                }
                plan
            }
        };
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::{
        operation::{ClaimToken, OperationRecord},
        task_graph::{TaskId, TaskState},
    };

    fn host() -> std::sync::Arc<KernelHost> {
        KernelHost::in_memory().0
    }

    fn seeded() -> ProcessSnapshot {
        ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap()
    }

    fn id_for(snapshot: &ProcessSnapshot, node: &str, sequence: u64) -> OperationId {
        OperationId {
            process_id: snapshot.process_id.clone(),
            task: TaskId::new(node, 0).unwrap(),
            sequence,
        }
    }

    fn commit(id: &OperationId) -> OperationCommit {
        OperationCommit {
            id: id.clone(),
            claim: ClaimToken {
                owner: "test".into(),
                fence: 1,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        }
    }

    fn operation<'a>(
        id: &'a OperationId,
        claim: &'a OperationCommit,
        snapshot: &'a ProcessSnapshot,
        lane: &'a mut CommandLane,
    ) -> Operation<'a> {
        Operation {
            id,
            claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane,
        }
    }

    /// A skip with no execution left is not a plan: the operation cannot name
    /// what it settles.
    #[tokio::test]
    async fn skip_without_an_execution_is_an_error() {
        let host = host();
        let snapshot = seeded();
        let id = id_for(&snapshot, "a", 2);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: None,
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let error = SkipTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("not found"), "{error}");
    }

    /// A skip settles a live execution and hands propagation the dead source.
    #[tokio::test]
    async fn skip_settles_a_live_execution() {
        let host = host();
        let mut snapshot = seeded();
        snapshot
            .tasks
            .get_mut(&TaskId::new("a", 0).unwrap())
            .unwrap()
            .state = TaskState::Running;
        let id = id_for(&snapshot, "a", 2);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = operation(&id, &claim, &snapshot, &mut lane);
        let plan = SkipTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap();
        assert_eq!(plan.state_changes.len(), 1);
        assert_eq!(plan.state_changes[0].to, TaskState::Skipped);
        assert_eq!(plan.outbox[0].topic, "task.skipped");
        assert_eq!(
            plan.operations[0].kind,
            OperationKind::PropagateTask,
            "the cascade continues through propagation"
        );
    }

    /// The propagation handed off already (a duplicate wake) is not re-created.
    #[tokio::test]
    async fn skip_does_not_duplicate_the_propagation() {
        let host = host();
        let mut snapshot = seeded();
        snapshot
            .tasks
            .get_mut(&TaskId::new("a", 0).unwrap())
            .unwrap()
            .state = TaskState::Waiting;
        let id = id_for(&snapshot, "a", 2);
        let next = OperationId {
            sequence: id.sequence + 1,
            ..id.clone()
        };
        snapshot.operations.insert(
            next.clone(),
            OperationRecord::pending(next, OperationKind::PropagateTask),
        );
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = operation(&id, &claim, &snapshot, &mut lane);
        let plan = SkipTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap();
        assert!(plan.operations.is_empty());
    }

    /// A skip only reaches `Ready`, `Running` and `Waiting`; a terminal
    /// execution is a decided one.
    #[tokio::test]
    async fn skip_refuses_a_terminal_execution() {
        let host = host();
        let mut snapshot = seeded();
        snapshot
            .tasks
            .get_mut(&TaskId::new("a", 0).unwrap())
            .unwrap()
            .state = TaskState::Completed;
        let id = id_for(&snapshot, "a", 2);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = operation(&id, &claim, &snapshot, &mut lane);
        let error = SkipTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap_err();
        assert!(
            format!("{error}").contains("expected Ready, Running or Waiting"),
            "{error}"
        );
    }

    /// Every cascade kind runs nothing: the snapshot is its whole input.
    #[tokio::test]
    async fn abort_and_skip_are_plan_only() {
        let host = host();
        let snapshot = seeded();
        let pending = Pending {
            id: &id_for(&snapshot, "a", 0),
            snapshot: &snapshot,
            now_ms: 0,
        };
        assert_eq!(
            SkipTask.precheck(&host, &pending).await.unwrap(),
            Precheck::Proceed
        );
        assert_eq!(
            AbortTask.precheck(&host, &pending).await.unwrap(),
            Precheck::Proceed
        );
        let id = id_for(&snapshot, "a", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let mut operation = operation(&id, &claim, &snapshot, &mut lane);
        assert!(matches!(
            SkipTask.run(&host, &mut operation).await.unwrap(),
            Outcome::Planned
        ));
        assert!(matches!(
            AbortTask.run(&host, &mut operation).await.unwrap(),
            Outcome::Planned
        ));
    }

    #[tokio::test]
    async fn abort_without_an_execution_is_an_error() {
        let host = host();
        let snapshot = seeded();
        let id = id_for(&snapshot, "a", 2);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: None,
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let error = AbortTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("not found"), "{error}");
    }

    /// A decision that landed while the abort was queued leaves the operation
    /// its bare remainder: no state change, no propagation.
    #[tokio::test]
    async fn abort_of_a_settled_execution_is_a_bare_remainder() {
        let host = host();
        let mut snapshot = seeded();
        snapshot
            .tasks
            .get_mut(&TaskId::new("a", 0).unwrap())
            .unwrap()
            .state = TaskState::Completed;
        let id = id_for(&snapshot, "a", 2);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = operation(&id, &claim, &snapshot, &mut lane);
        let plan = AbortTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap();
        assert!(plan.state_changes.is_empty());
        assert!(plan.operations.is_empty());
    }

    /// A running execution is decided `Aborted` and propagates the decision.
    #[tokio::test]
    async fn abort_settles_a_running_execution() {
        let host = host();
        let mut snapshot = seeded();
        snapshot
            .tasks
            .get_mut(&TaskId::new("a", 0).unwrap())
            .unwrap()
            .state = TaskState::Running;
        let id = id_for(&snapshot, "a", 2);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = operation(&id, &claim, &snapshot, &mut lane);
        let plan = AbortTask
            .next(&host, &operation, Outcome::Planned)
            .await
            .unwrap();
        assert_eq!(plan.state_changes.len(), 1);
        assert_eq!(plan.state_changes[0].to, TaskState::Aborted);
        assert_eq!(plan.outbox[0].topic, "task.aborted");
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    }
}
