//! The kernel host: the shell-side driver of the deterministic kernel.
//!
//! It owns no state of its own — every fact lives in the aggregate documents.
//! Wakes are a plain channel and strictly an optimization: losing one is safe
//! because `recover` re-derives the same work from the recovery index. The
//! host drives one operation kind per durable hop:
//!
//! - `ExecuteTask` commits `Ready → Running` (the task started) and seeds the
//!   `ApplyAction` operation whose effect performs the task's work. The
//!   package effect itself arrives in P2; until then the host auto-completes.
//! - `ApplyAction` commits `Running → Completed` (the effect is durable) and
//!   seeds the `PropagateTask` operation in the same atomic batch — a crash
//!   between effect and propagation leaves the operation open, never the
//!   reverse.
//! - `PropagateTask` re-plans propagation from the authoritative snapshot and
//!   commits the successors' `Created → Ready` plus their `ExecuteTask`
//!   operations; join readiness omits waiting successors idempotently.
//!
//! Every hop is one `compare_and_commit` with the hop's own operation as the
//! `source_operation`, so the operation is committed inside the same atomic
//! batch as its business effect. When a hop's durable remainder already
//! exists (a wake redelivered after the commit), the hop commits an empty
//! plan sourced by its own operation, which completes the operation without
//! touching business state.

use self::model::{ProcessSpec, execution_graph};
use crate::kernel::{
    AggregateDocumentRepository, AggregateRepository, AggregateSnapshot, AtomicDocumentStore,
    ClaimToken, Clock, DispatchBatchConfig, DispatchBatchReport, ExecutionId, ExecutionState,
    JoinPolicy, LoadedAggregate, MemoryDocumentStore, OperationCommit, OperationId,
    OperationIntent, OperationKind, OperationRepository, OperationState, OperationWakeSink,
    OutboxIntent, OutboxTransport, PropagateCommand, PropagationPlanner, RecoveryReport,
    RecoveryService, StateChange, SystemClock, TransitionPlan, WakeError, dispatch_batch,
};
use crate::{ActError, Result};
use std::sync::mpsc::{Receiver, Sender, channel};

mod model;

fn runtime(error: impl std::fmt::Display) -> ActError {
    ActError::Runtime(error.to_string())
}

const OWNER: &str = "kernel-host";
const LEASE_MS: i64 = 60_000;

/// Best-effort wake delivery into the host's queue. Losing a wake is safe:
/// the recovery scan re-derives the same pending operations.
#[derive(Clone)]
struct QueueWake {
    tx: Sender<OperationId>,
}

impl OperationWakeSink for QueueWake {
    fn wake(&mut self, operation: &OperationId) -> std::result::Result<(), WakeError> {
        self.tx
            .send(operation.clone())
            .map_err(|error| WakeError(error.to_string()))
    }
}

/// What one quiescence pass did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunReport {
    pub(crate) executed: usize,
    pub(crate) retried: usize,
    pub(crate) ignored: usize,
}

pub(crate) struct KernelHost<S: AtomicDocumentStore> {
    repository: AggregateDocumentRepository<S>,
    wake_tx: Sender<OperationId>,
    wake_rx: Receiver<OperationId>,
}

impl KernelHost<MemoryDocumentStore> {
    /// An in-memory host for tests and for the P2 parity harness.
    pub(crate) fn in_memory() -> Self {
        Self::new(MemoryDocumentStore::default())
    }
}

impl<S: AtomicDocumentStore> KernelHost<S> {
    pub(crate) fn new(store: S) -> Self {
        let (wake_tx, wake_rx) = channel();
        Self {
            repository: AggregateDocumentRepository::new(store),
            wake_tx,
            wake_rx,
        }
    }

    /// Create the process atomically and wake its root operation.
    pub(crate) fn start_process(&mut self, spec: &ProcessSpec) -> Result<String> {
        let snapshot = spec.seed_snapshot()?;
        self.repository.create(&snapshot).map_err(runtime)?;
        let root = OperationId {
            process_id: spec.process_id.clone(),
            task_id: spec.root.clone(),
            generation: 0,
            sequence: 0,
        };
        QueueWake {
            tx: self.wake_tx.clone(),
        }
        .wake(&root)
        .map_err(|error| ActError::Runtime(error.0))?;
        Ok(spec.process_id.clone())
    }

    /// Drive woken operations until the queue drains or `max_steps` hops ran.
    pub(crate) fn run_until_quiescent(&mut self, max_steps: usize) -> Result<RunReport> {
        let mut report = RunReport::default();
        for _ in 0..max_steps {
            let id = match self.wake_rx.try_recv() {
                Ok(id) => id,
                Err(_) => break,
            };
            match self.execute_one(&id) {
                Ok(true) => report.executed += 1,
                Ok(false) => report.ignored += 1,
                Err(error) => {
                    report.retried += 1;
                    #[cfg(test)]
                    eprintln!("hop {id:?} failed: {error}");
                    tracing::debug!(operation = ?id, error = %error, "hop failed; operation stays open for recovery");
                }
            }
        }
        Ok(report)
    }

    /// One durable hop. Returns `Ok(false)` when the operation needed nothing
    /// (a wake redelivered for an already-terminal operation).
    fn execute_one(&mut self, id: &OperationId) -> Result<bool> {
        let now = SystemClock.now_ms();
        let snapshot = self
            .load(&id.process_id)?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        let record = snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("operation {id:?} not found")))?;
        match record.state {
            OperationState::Pending | OperationState::RetryScheduled { .. } => {}
            _ => return Ok(false),
        }

        let claim = ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let claimed = self
            .repository
            .claim_operation(id, record.revision, claim.clone())
            .map_err(runtime)?;
        let prepared = self
            .repository
            .prepare_operation(id, claimed.revision, claim.clone())
            .map_err(runtime)?;
        debug_assert_eq!(
            prepared.state,
            OperationState::CommitPrepared(claim.clone())
        );

        // Re-read after prepare: ownership moved the aggregate version, so the
        // plan must be formed from the authoritative snapshot.
        let snapshot = self
            .load(&id.process_id)?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        let execution_id = ExecutionId {
            task_id: id.task_id.clone(),
            generation: id.generation,
        };
        let execution = snapshot.executions.get(&execution_id).cloned();

        let plan = match record.kind {
            OperationKind::ExecuteTask => {
                let action_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let action_exists = snapshot.operations.contains_key(&action_id);
                let mut plan = self.task_hop_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    execution.as_ref(),
                    &claim,
                    "started",
                    "task.started",
                )?;
                if !action_exists {
                    plan.operations.push(OperationIntent {
                        id: action_id,
                        kind: OperationKind::ApplyAction,
                    });
                }
                if let Some(ready) = execution
                    .as_ref()
                    .filter(|ready| ready.state == ExecutionState::Ready)
                {
                    plan.state_changes.push(StateChange {
                        execution: execution_id.clone(),
                        expected_revision: ready.revision,
                        from: ExecutionState::Ready,
                        to: ExecutionState::Running,
                    });
                }
                plan
            }
            OperationKind::ApplyAction => {
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if execution.state != ExecutionState::Running {
                    return Err(ActError::Runtime(format!(
                        "execution {execution_id:?} is {:?}, expected Running",
                        execution.state
                    )));
                }
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let mut plan = self.task_hop_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    Some(&execution),
                    &claim,
                    "completed",
                    "task.completed",
                )?;
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: ExecutionState::Running,
                    to: ExecutionState::Completed,
                });
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                    });
                }
                plan
            }
            OperationKind::PropagateTask => {
                // The graph of record is the snapshot itself (parent map +
                // predecessor lists), never the declaring spec.
                let graph = execution_graph(&snapshot)?;
                let command = PropagateCommand {
                    process_id: id.process_id.clone(),
                    execution: execution_id.clone(),
                    source_operation: OperationCommit {
                        id: id.clone(),
                        claim: claim.clone(),
                    },
                    join_policy: JoinPolicy::AllSucceeded,
                };
                PropagationPlanner::plan(&graph, &snapshot, &command).map_err(runtime)?
            }
            other => {
                return Err(ActError::Runtime(format!(
                    "operation kind {other:?} is not driven by the host yet"
                )));
            }
        };

        let receipt = self.repository.compare_and_commit(&plan).map_err(runtime)?;
        let committed = self
            .load(&id.process_id)?
            .ok_or_else(|| ActError::Runtime("aggregate lost after commit".into()))?
            .snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime("operation lost after commit".into()))?;
        self.repository
            .complete_operation(id, committed.revision, receipt.token.clone())
            .map_err(runtime)?;
        for operation in &receipt.new_operations {
            QueueWake {
                tx: self.wake_tx.clone(),
            }
            .wake(operation)
            .map_err(|error| ActError::Runtime(error.0))?;
        }
        Ok(true)
    }

    /// The empty-sourced hop plan: outbox note plus (when absent) nothing
    /// else. The operation's own commit is the only state delta.
    #[allow(clippy::too_many_arguments)]
    fn task_hop_plan(
        &self,
        snapshot: &AggregateSnapshot,
        id: &OperationId,
        execution_id: &ExecutionId,
        execution: Option<&crate::kernel::ExecutionSnapshot>,
        claim: &ClaimToken,
        note: &str,
        topic: &str,
    ) -> Result<TransitionPlan> {
        execution
            .ok_or_else(|| ActError::Runtime(format!("execution {execution_id:?} not found")))?;
        Ok(TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(OperationCommit {
                id: id.clone(),
                claim: claim.clone(),
            }),
            operations: Vec::new(),
            outbox: vec![OutboxIntent {
                key: format!("{note}:{}:{}:{}", id.process_id, id.task_id, id.generation),
                topic: topic.into(),
                payload: String::new(),
            }],
        })
    }

    /// Re-derive pending work from the recovery index. The single source of
    /// truth after a crash: every woken operation is durable and non-terminal.
    pub(crate) fn recover(&mut self, now_ms: i64) -> Result<RecoveryReport> {
        let mut sink = QueueWake {
            tx: self.wake_tx.clone(),
        };
        let mut service = RecoveryService::new(&mut self.repository, &mut sink);
        service.scan(now_ms, 128).map_err(runtime)
    }

    /// One outbox dispatch pass with the caller's transport.
    pub(crate) fn dispatch_outbox(
        &mut self,
        transport: &mut impl OutboxTransport,
    ) -> Result<DispatchBatchReport> {
        let config = DispatchBatchConfig {
            owner: OWNER.into(),
            lease_ms: 30_000,
            page_size: 128,
            max_records: 10_000,
        };
        dispatch_batch(&mut self.repository, transport, &SystemClock, &config)
            .map_err(|error| ActError::Runtime(error.to_string()))
    }

    fn load(&mut self, process_id: &str) -> Result<Option<LoadedAggregate>> {
        self.repository.load(process_id).map_err(runtime)
    }

    pub(crate) fn snapshot(&mut self, process_id: &str) -> Result<Option<AggregateSnapshot>> {
        Ok(self.load(process_id)?.map(|loaded| loaded.snapshot))
    }

    /// Whether every execution of the process reached a terminal state.
    pub(crate) fn process_completed(&mut self, process_id: &str) -> Result<bool> {
        let Some(snapshot) = self.snapshot(process_id)? else {
            return Ok(false);
        };
        Ok(snapshot
            .executions
            .values()
            .all(|execution| execution.state.is_terminal()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{DeliveryOutcome, OutboxRecord, OutboxState};

    struct RecordingTransport {
        delivered: usize,
    }

    impl OutboxTransport for RecordingTransport {
        fn deliver(&mut self, _record: &OutboxRecord) -> DeliveryOutcome {
            self.delivered += 1;
            DeliveryOutcome::Delivered {
                receipt: "ok".into(),
            }
        }
    }

    fn assert_all_terminal(host: &mut KernelHost<MemoryDocumentStore>, pid: &str) {
        let snapshot = host.snapshot(pid).unwrap().expect("aggregate exists");
        assert!(!snapshot.executions.is_empty());
        for (id, execution) in &snapshot.executions {
            assert!(
                execution.state.is_terminal(),
                "execution {id:?} stuck at {:?}",
                execution.state
            );
        }
        for (id, record) in &snapshot.operations {
            assert!(
                matches!(record.state, OperationState::Completed(_)),
                "operation {id:?} stuck at {:?}",
                record.state
            );
        }
    }

    #[test]
    fn linear_process_runs_to_completion_and_delivers_outbox() {
        let mut host = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p1", "root", &["a", "b"]);
        host.start_process(&spec).unwrap();
        let report = host.run_until_quiescent(100).unwrap();
        // 3 nodes x (execute + action + propagate) hops, root included.
        assert_eq!(report.executed, 9, "nine hops expected: {report:?}");
        assert_eq!(report.retried, 0);
        assert_all_terminal(&mut host, "p1");

        let snapshot = host.snapshot("p1").unwrap().unwrap();
        // started x3 + completed x3 + propagated x3.
        assert_eq!(snapshot.outbox.len(), 9);

        let mut transport = RecordingTransport { delivered: 0 };
        let batch = host.dispatch_outbox(&mut transport).unwrap();
        assert_eq!(batch.delivered.len(), 9);
        assert_eq!(transport.delivered, 9);
        let snapshot = host.snapshot("p1").unwrap().unwrap();
        assert!(
            snapshot
                .outbox
                .values()
                .all(|record| matches!(record.state, OutboxState::Delivered { .. }))
        );
    }

    #[test]
    fn wake_loss_is_recovered_by_the_scan() {
        let mut host = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p2", "root", &["only"]);
        // Create without waking: everything now rides on the recovery scan,
        // exactly like after a crash that lost the in-memory queue.
        let snapshot = spec.seed_snapshot().unwrap();
        host.repository.create(&snapshot).map_err(runtime).unwrap();
        let now = SystemClock.now_ms();
        let report = host.recover(now).unwrap();
        assert!(
            !report.woken.is_empty(),
            "the pending root operation must be re-derived: {report:?}"
        );
        host.run_until_quiescent(100).unwrap();
        assert!(host.process_completed("p2").unwrap());
    }

    // A two-predecessor join is out of P1 scope: the flat model graph is a
    // pure successor chain, and a join with genuinely parallel branches needs
    // P2's `ExecutionCreate`-driven branch planning. The join readiness code
    // itself stays covered by `kernel::propagation` tests.
}
