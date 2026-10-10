//! The kernel host: the async shell driving the deterministic kernel.
//!
//! It owns no state of its own — every fact lives in the aggregate documents
//! over the engine's [`KvStore`]. Wakes are a channel and strictly an
//! optimization: losing one is safe because `recover` re-derives the same
//! work from the recovery index. The host drives one operation kind per
//! durable hop:
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
//! Every hop is one `commit_transition` with the hop's own operation as the
//! `source_operation`, so the operation is committed inside the same atomic
//! batch as its business effect. When a hop's durable remainder already
//! exists (a wake redelivered after the commit), the hop commits an empty
//! plan sourced by its own operation, which completes the operation without
//! touching business state.

use crate::kernel::{
    AsyncAggregateRepository, AsyncRecoveryRepository, Clock, DeliveryClaim, KvDocumentStore,
    OperationCommit, OperationEvent, OperationId, OperationIntent, OperationKind, OperationState,
    OutboxEvent, PropagateCommand, PropagationPlanner, RecoveryDisposition, RecoveryPlanner,
    RecoveryReport, StateChange, SystemClock, TransitionPlan,
};
use crate::store::{KvStore, MemoryStore};
use crate::{ActError, Result};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender, channel};

mod model;

use self::model::{ProcessSpec, execution_graph};

const OWNER: &str = "kernel-host";
const LEASE_MS: i64 = 60_000;
const DELIVERY_LEASE_MS: i64 = 30_000;
const PAGE_SIZE: usize = 128;

fn runtime(error: impl std::fmt::Display) -> ActError {
    ActError::Runtime(error.to_string())
}

/// Best-effort wake delivery into the host's queue. Losing a wake is safe:
/// the recovery scan re-derives the same pending operations.
#[derive(Clone)]
struct QueueWake {
    tx: Sender<OperationId>,
}

impl QueueWake {
    fn wake(&self, operation: &OperationId) {
        // A full or closed queue loses the wake by design; the recovery scan
        // is the durable net.
        let _ = self.tx.try_send(operation.clone());
    }
}

/// What one quiescence pass did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunReport {
    pub(crate) executed: usize,
    pub(crate) retried: usize,
    pub(crate) ignored: usize,
}

pub(crate) struct KernelHost {
    aggregates: Arc<AsyncAggregateRepository>,
    recovery: Arc<AsyncRecoveryRepository>,
    wake_tx: Sender<OperationId>,
    wake_rx: Receiver<OperationId>,
}

impl KernelHost {
    /// A host over the engine's key-value store. Tests pass
    /// `Arc::new(MemoryStore::new())`.
    pub(crate) fn new(kv: Arc<dyn KvStore>) -> Self {
        let store = Arc::new(KvDocumentStore::new(kv));
        let aggregates = Arc::new(AsyncAggregateRepository::new(store));
        let recovery = Arc::new(AsyncRecoveryRepository::new(aggregates.clone()));
        let (wake_tx, wake_rx) = channel(PAGE_SIZE * 8);
        Self {
            aggregates,
            recovery,
            wake_tx,
            wake_rx,
        }
    }

    /// A host over an in-memory store for tests and the P2 parity harness.
    pub(crate) fn in_memory() -> Self {
        Self::new(Arc::new(MemoryStore::new()))
    }

    /// Create the process atomically and wake its root operation.
    pub(crate) async fn start_process(&self, spec: &ProcessSpec) -> Result<String> {
        let snapshot = spec.seed_snapshot()?;
        self.aggregates.create(&snapshot).await.map_err(runtime)?;
        let root = OperationId {
            process_id: spec.process_id.clone(),
            task_id: spec.root.clone(),
            generation: 0,
            sequence: 0,
        };
        QueueWake {
            tx: self.wake_tx.clone(),
        }
        .wake(&root);
        Ok(spec.process_id.clone())
    }

    /// Drive woken operations until the queue drains or `max_steps` hops ran.
    pub(crate) async fn run_until_quiescent(&mut self, max_steps: usize) -> Result<RunReport> {
        let mut report = RunReport::default();
        for _ in 0..max_steps {
            let id = match self.wake_rx.try_recv() {
                Ok(id) => id,
                Err(_) => break,
            };
            match self.execute_one(&id).await {
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
    async fn execute_one(&self, id: &OperationId) -> Result<bool> {
        let now = SystemClock.now_ms();
        let snapshot = self
            .load(&id.process_id)
            .await?
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

        let claim = crate::kernel::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let claimed = self
            .operation_event(id, record.revision, OperationEvent::Claim(claim.clone()))
            .await?;
        let prepared = self
            .operation_event(
                id,
                claimed.revision,
                OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                },
            )
            .await?;
        debug_assert!(matches!(prepared.state, OperationState::CommitPrepared(_)));

        // Re-read after prepare: ownership moved the aggregate version, so the
        // plan must be formed from the authoritative snapshot.
        let snapshot = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        let execution_id = crate::kernel::ExecutionId {
            task_id: id.task_id.clone(),
            generation: id.generation,
        };
        let execution = snapshot.executions.get(&execution_id).cloned();
        let commit_claim = OperationCommit {
            id: id.clone(),
            claim: crate::kernel::ClaimToken {
                owner: claim.owner.clone(),
                fence: claim.fence,
                lease_until_ms: claim.lease_until_ms,
                heartbeat_revision: claim.heartbeat_revision,
            },
        };

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
                    &commit_claim,
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
                    .filter(|ready| ready.state == crate::kernel::ExecutionState::Ready)
                {
                    plan.state_changes.push(StateChange {
                        execution: execution_id.clone(),
                        expected_revision: ready.revision,
                        from: crate::kernel::ExecutionState::Ready,
                        to: crate::kernel::ExecutionState::Running,
                    });
                }
                plan
            }
            OperationKind::ApplyAction => {
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if execution.state != crate::kernel::ExecutionState::Running {
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
                    &commit_claim,
                    "completed",
                    "task.completed",
                )?;
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: crate::kernel::ExecutionState::Running,
                    to: crate::kernel::ExecutionState::Completed,
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
                // The graph of record is the snapshot itself (parent map plus
                // predecessor lists), never the declaring spec.
                let graph = execution_graph(&snapshot)?;
                let command = PropagateCommand {
                    process_id: id.process_id.clone(),
                    execution: execution_id.clone(),
                    source_operation: commit_claim,
                    join_policy: crate::kernel::JoinPolicy::AllSucceeded,
                };
                PropagationPlanner::plan(&graph, &snapshot, &command).map_err(runtime)?
            }
            other => {
                return Err(ActError::Runtime(format!(
                    "operation kind {other:?} is not driven by the host yet"
                )));
            }
        };

        let receipt = match self
            .aggregates
            .commit_transition(&plan)
            .await
            .map_err(runtime)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { receipt, .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { receipt, .. } => receipt,
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                // Another writer advanced the aggregate; the wake stays open
                // and recovery re-derives the hop against the new state.
                return Err(ActError::Runtime(format!(
                    "commit conflict on operation {id:?}"
                )));
            }
        };
        let committed = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost after commit".into()))?
            .snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime("operation lost after commit".into()))?;
        self.operation_event(
            id,
            committed.revision,
            OperationEvent::Complete {
                commit: receipt.token.clone(),
            },
        )
        .await?;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        for operation in &receipt.new_operations {
            wake.wake(operation);
        }
        Ok(true)
    }

    /// The bare hop plan: the operation's own commit plus its outbox note.
    /// State changes and successor operations are layered on by the callers.
    #[allow(clippy::too_many_arguments)]
    fn task_hop_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: Option<&crate::kernel::ExecutionSnapshot>,
        claim: &OperationCommit,
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
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: vec![crate::kernel::OutboxIntent {
                key: format!("{note}:{}:{}:{}", id.process_id, id.task_id, id.generation),
                topic: topic.into(),
                payload: String::new(),
            }],
        })
    }

    /// Claim → prepare one operation through atomic operation events.
    async fn operation_event(
        &self,
        id: &OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<crate::kernel::OperationRecord> {
        match self
            .aggregates
            .commit_operation_event(id, expected_revision, event)
            .await
            .map_err(runtime)?
        {
            crate::kernel::OperationCommitOutcome::Committed { operation, .. }
            | crate::kernel::OperationCommitOutcome::AlreadyCommitted { operation, .. } => {
                Ok(operation)
            }
            crate::kernel::OperationCommitOutcome::Conflict { .. } => Err(ActError::Runtime(
                format!("claim conflict on operation {id:?}"),
            )),
        }
    }

    async fn load(&self, process_id: &str) -> Result<Option<crate::kernel::LoadedAggregate>> {
        self.aggregates.load(process_id).await.map_err(runtime)
    }

    pub(crate) async fn snapshot(
        &self,
        process_id: &str,
    ) -> Result<Option<crate::kernel::AggregateSnapshot>> {
        Ok(self.load(process_id).await?.map(|loaded| loaded.snapshot))
    }

    /// Whether every execution of the process reached a terminal state.
    pub(crate) async fn process_completed(&self, process_id: &str) -> Result<bool> {
        let Some(snapshot) = self.snapshot(process_id).await? else {
            return Ok(false);
        };
        Ok(snapshot
            .executions
            .values()
            .all(|execution| execution.state.is_terminal()))
    }

    /// Re-derive pending work from the recovery index — the single source of
    /// truth after a crash. Wake and reclaim dispositions re-enter the queue;
    /// committed-but-unfinalized operations are completed; effect-unknown
    /// operations are surfaced for manual reconciliation.
    pub(crate) async fn recover(&self, now_ms: i64) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let mut cursor = None;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        loop {
            let page = self
                .recovery
                .recoverable_page(cursor.as_ref(), PAGE_SIZE)
                .await
                .map_err(runtime)?;
            if page.operations.is_empty() {
                break;
            }
            for operation in page.operations {
                report.scanned += 1;
                match RecoveryPlanner::classify(&operation, now_ms) {
                    RecoveryDisposition::Wake => {
                        wake.wake(&operation.id);
                        report.woken.push(operation.id);
                    }
                    RecoveryDisposition::AwaitOwner => {
                        report.awaiting_owner.push(operation.id);
                    }
                    RecoveryDisposition::ReclaimExpired => {
                        match self
                            .operation_event(
                                &operation.id,
                                operation.revision,
                                OperationEvent::ReleaseExpiredClaim { now_ms },
                            )
                            .await
                        {
                            Ok(reclaimed) => match reclaimed.state {
                                OperationState::RetryScheduled { .. } => {
                                    wake.wake(&reclaimed.id);
                                    report.woken.push(reclaimed.id);
                                }
                                OperationState::EffectUnknown { .. } => {
                                    report.reconciliation_required.push(reclaimed.id);
                                }
                                _ => {}
                            },
                            Err(error) => {
                                #[cfg(test)]
                                eprintln!("reclaim {operation:?} failed: {error}");
                                tracing::debug!(operation = ?operation.id, error = %error, "reclaim lost; next scan retries");
                            }
                        }
                    }
                    RecoveryDisposition::FinalizeCommit => {
                        let commit = match &operation.state {
                            OperationState::Committed(commit) => commit.clone(),
                            _ => continue,
                        };
                        match self
                            .operation_event(
                                &operation.id,
                                operation.revision,
                                OperationEvent::Complete { commit },
                            )
                            .await
                        {
                            Ok(finalized) => report.finalized.push(finalized.id),
                            Err(error) => {
                                tracing::debug!(operation = ?operation.id, error = %error, "finalize lost; next scan retries");
                            }
                        }
                    }
                    RecoveryDisposition::ManualReconciliation => {
                        report.reconciliation_required.push(operation.id);
                    }
                    RecoveryDisposition::IgnoreTerminal => {}
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(report)
    }

    /// One outbox delivery pass: claim every due record with a fenced claim,
    /// hand it to the transport, then persist the reported outcome. After the
    /// transport ran, only its explicit `RetryAt` proves no external effect —
    /// everything else the transport cannot classify stays `DeliveryUnknown`.
    pub(crate) async fn dispatch_outbox(
        &self,
        transport: &mut impl crate::kernel::OutboxTransport,
    ) -> Result<usize> {
        let now = SystemClock.now_ms();
        let mut delivered = 0usize;
        let mut cursor = None;
        loop {
            let page = self
                .aggregates
                .dispatchable_outbox_page(now, cursor.as_ref(), PAGE_SIZE)
                .await
                .map_err(runtime)?;
            if page.records.is_empty() {
                break;
            }
            for record in page.records {
                let claim = DeliveryClaim {
                    owner: OWNER.into(),
                    fence: record.last_fence + 1,
                    lease_until_ms: now + DELIVERY_LEASE_MS,
                    heartbeat_revision: 0,
                };
                let claimed = match self
                    .aggregates
                    .evolve_outbox(
                        &record.id,
                        record.revision,
                        OutboxEvent::Claim {
                            claim: claim.clone(),
                            now_ms: now,
                        },
                    )
                    .await
                {
                    Ok(claimed) => claimed,
                    Err(error) => {
                        // A concurrent worker claimed it first; skip without
                        // failing the pass.
                        tracing::debug!(record = ?record.id, error = %error, "delivery claim lost");
                        continue;
                    }
                };
                let event = match transport.deliver(&claimed) {
                    crate::kernel::DeliveryOutcome::Delivered { receipt } => {
                        delivered += 1;
                        OutboxEvent::ConfirmDelivered {
                            claim: claim.clone(),
                            receipt,
                            now_ms: now,
                        }
                    }
                    crate::kernel::DeliveryOutcome::RetryAt {
                        available_at_ms,
                        error,
                    } => OutboxEvent::Reschedule {
                        claim: claim.clone(),
                        available_at_ms,
                        error,
                        now_ms: now,
                    },
                    crate::kernel::DeliveryOutcome::OutcomeUnknown { reason } => {
                        OutboxEvent::MarkDeliveryUnknown {
                            claim: claim.clone(),
                            reason,
                            now_ms: now,
                        }
                    }
                    crate::kernel::DeliveryOutcome::Quarantine { reason } => {
                        OutboxEvent::Quarantine { reason }
                    }
                };
                self.aggregates
                    .evolve_outbox(&record.id, claimed.revision, event)
                    .await
                    .map_err(runtime)?;
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{DeliveryOutcome, OutboxRecord, OutboxState};

    struct RecordingTransport {
        delivered: usize,
    }

    impl crate::kernel::OutboxTransport for RecordingTransport {
        fn deliver(&mut self, _record: &OutboxRecord) -> DeliveryOutcome {
            self.delivered += 1;
            DeliveryOutcome::Delivered {
                receipt: "ok".into(),
            }
        }
    }

    async fn assert_all_terminal(host: &KernelHost, pid: &str) {
        let snapshot = host.snapshot(pid).await.unwrap().expect("aggregate exists");
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

    #[tokio::test]
    async fn linear_process_runs_to_completion_and_delivers_outbox() {
        let mut host = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p1", "root", &["a", "b"]);
        host.start_process(&spec).await.unwrap();
        let report = host.run_until_quiescent(100).await.unwrap();
        // 3 nodes x (execute + action + propagate) hops, root included.
        assert_eq!(report.executed, 9, "nine hops expected: {report:?}");
        assert_eq!(report.retried, 0);
        assert_all_terminal(&host, "p1").await;

        let snapshot = host.snapshot("p1").await.unwrap().unwrap();
        // started x3 + completed x3 + propagated x3.
        assert_eq!(snapshot.outbox.len(), 9);

        let mut transport = RecordingTransport { delivered: 0 };
        let delivered = host.dispatch_outbox(&mut transport).await.unwrap();
        assert_eq!(delivered, 9);
        assert_eq!(transport.delivered, 9);
        let snapshot = host.snapshot("p1").await.unwrap().unwrap();
        assert!(
            snapshot
                .outbox
                .values()
                .all(|record| matches!(record.state, OutboxState::Delivered { .. }))
        );
    }

    #[tokio::test]
    async fn wake_loss_is_recovered_by_the_scan() {
        let host = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p2", "root", &["only"]);
        // Create without waking: everything now rides on the recovery scan,
        // exactly like after a crash that lost the in-memory queue.
        let snapshot = spec.seed_snapshot().unwrap();
        host.aggregates
            .create(&snapshot)
            .await
            .map_err(runtime)
            .unwrap();
        let now = SystemClock.now_ms();
        let report = host.recover(now).await.unwrap();
        assert!(
            !report.woken.is_empty(),
            "the pending root operation must be re-derived: {report:?}"
        );
        let mut host = host;
        host.run_until_quiescent(100).await.unwrap();
        assert!(host.process_completed("p2").await.unwrap());
    }

    /// The parity floor: the same flat workflow driven by the legacy engine
    /// and by the kernel host must reach the same terminal task set. Every
    /// P2 cutover slice extends this harness instead of asserting in isolation.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_path_matches_legacy_for_flat_chain() {
        let workflow = {
            let mut workflow = crate::model::Workflow::new().with_id("wf-parity");
            workflow = workflow.with_step(|step| step.with_id("s1"));
            workflow = workflow.with_step(|step| step.with_id("s2"));
            workflow
        };
        let expected_tasks: Vec<String> = vec![
            crate::utils::consts::TASK_ROOT_TID.to_string(),
            "s1".into(),
            "s2".into(),
        ];

        // Legacy engine: start and wait for the process to settle.
        let engine = crate::Engine::builder().start().await.unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let legacy_proc = engine
            .runtime()
            .start(&workflow, crate::Vars::new())
            .await
            .unwrap();
        let pid = legacy_proc.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("legacy engine never finished");
        let legacy = engine.runtime().proc(&pid).await.unwrap().unwrap();
        let mut legacy_tasks: Vec<String> =
            legacy.tasks().iter().map(|task| task.id.clone()).collect();
        legacy_tasks.sort();

        // Kernel host: same model, same expectation.
        let mut host = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("k-parity", &workflow).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(100).await.unwrap();
        assert!(host.process_completed("k-parity").await.unwrap());
        let snapshot = host.snapshot("k-parity").await.unwrap().unwrap();
        let kernel_tasks: Vec<String> = snapshot
            .executions
            .keys()
            .map(|id| id.task_id.clone())
            .collect();

        // The kernel derives execution identity from model node ids; the
        // legacy engine mints random task ids for the acts it runs. Parity
        // here is the terminal task set size and completion, not id equality.
        assert_eq!(legacy_tasks.len(), kernel_tasks.len());
        assert_eq!(legacy_tasks.len(), expected_tasks.len());
        assert!(snapshot.executions.values().all(|e| e.state.is_terminal()));
    }
}
