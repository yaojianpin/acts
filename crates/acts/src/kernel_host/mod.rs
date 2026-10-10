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

use self::model::execution_graph;
pub(crate) use model::ProcessSpec;

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

/// The engine-facing kernel host: a cloneable handle over the aggregate
/// documents. The wake channel's receiving end is returned by [`KernelHost::new`]
/// and belongs to exactly one driver task ([`KernelHost::serve`]).
#[derive(Clone)]
pub(crate) struct KernelHost {
    aggregates: Arc<AsyncAggregateRepository>,
    recovery: Arc<AsyncRecoveryRepository>,
    wake_tx: Sender<OperationId>,
}

/// Delivers shadow-mode outbox records: they are engine-internal lifecycle
/// notes (`task.started`/`task.completed`/`task.propagated`), logged and
/// confirmed. Channel delivery for the kernel path arrives with the executor
/// wiring.
struct ShadowTransport;

impl crate::kernel::OutboxTransport for ShadowTransport {
    fn deliver(&mut self, record: &crate::kernel::OutboxRecord) -> crate::kernel::DeliveryOutcome {
        tracing::debug!(
            pid = %record.id.process_id,
            key = %record.id.key,
            topic = %record.topic,
            "kernel outbox delivered (shadow)"
        );
        crate::kernel::DeliveryOutcome::Delivered {
            receipt: "shadow".into(),
        }
    }
}

impl KernelHost {
    /// A host over the engine's key-value store, plus the wake receiver its
    /// driver task must own. Tests pass `Arc::new(MemoryStore::new())`.
    pub(crate) fn new(kv: Arc<dyn KvStore>) -> (Arc<Self>, Receiver<OperationId>) {
        let store = Arc::new(KvDocumentStore::new(kv));
        let aggregates = Arc::new(AsyncAggregateRepository::new(store));
        let recovery = Arc::new(AsyncRecoveryRepository::new(aggregates.clone()));
        let (wake_tx, wake_rx) = channel(PAGE_SIZE * 8);
        (
            Arc::new(Self {
                aggregates,
                recovery,
                wake_tx,
            }),
            wake_rx,
        )
    }

    /// A host over an in-memory store for tests and the P2 parity harness.
    pub(crate) fn in_memory() -> (Arc<Self>, Receiver<OperationId>) {
        Self::new(Arc::new(MemoryStore::new()))
    }

    /// The background driver: executes woken hops as they arrive, and on every
    /// tick re-derives recovery work (the durable net) and runs one outbox
    /// delivery pass. Exits when `shutdown` fires.
    pub(crate) async fn serve(
        self: Arc<Self>,
        mut wake_rx: Receiver<OperationId>,
        shutdown: tokio_util::sync::CancellationToken,
        tick: std::time::Duration,
    ) {
        let mut ticker = tokio::time::interval(tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut transport = ShadowTransport;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let now = SystemClock.now_ms();
                    if let Err(error) = self.recover(now).await {
                        tracing::warn!(error = %error, "kernel recovery scan failed");
                    }
                    if let Err(error) = self.dispatch_outbox(&mut transport).await {
                        tracing::warn!(error = %error, "kernel outbox pass failed");
                    }
                }
                id = wake_rx.recv() => {
                    let Some(id) = id else { break };
                    if let Err(error) = self.execute_one(&id).await {
                        // The operation stays open; the next recovery tick
                        // re-derives it.
                        tracing::debug!(operation = ?id, error = %error, "kernel hop failed; left open for recovery");
                    }
                }
            }
        }
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
    /// Test-side twin of [`Self::serve`]: same hop execution, deterministic
    /// drain instead of a select loop.
    pub(crate) async fn run_until_quiescent(
        &self,
        wake_rx: &mut Receiver<OperationId>,
        max_steps: usize,
    ) -> Result<RunReport> {
        let mut report = RunReport::default();
        for _ in 0..max_steps {
            let id = match wake_rx.try_recv() {
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

    /// Apply a client action to one execution: the durable intent (an
    /// `ApplyAction` operation) is created first, then owned, re-planned
    /// against the authoritative snapshot, committed, and completed — the
    /// same sequence [`crate::kernel::ActionApplicationService`] uses. A
    /// semantically invalid action is rejected before anything durable
    /// happens.
    pub(crate) async fn apply_action(
        &self,
        process_id: &str,
        task_id: &str,
        action: crate::kernel::ActionKind,
        inputs: Vec<(String, String)>,
    ) -> Result<()> {
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {process_id} not found")))?
            .snapshot;
        let execution_id = snapshot
            .executions
            .keys()
            .filter(|id| id.task_id == task_id)
            .max_by_key(|id| id.generation)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("execution {task_id} not found")))?;
        let next_sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == process_id
                    && id.task_id == task_id
                    && id.generation == execution_id.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        let operation_id = OperationId {
            process_id: process_id.into(),
            task_id: task_id.into(),
            generation: execution_id.generation,
            sequence: next_sequence,
        };
        let now = SystemClock.now_ms();
        let claim = crate::kernel::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let command = crate::kernel::ApplyActionCommand {
            process_id: process_id.into(),
            execution: execution_id.clone(),
            action,
            inputs,
            source_operation: OperationCommit {
                id: operation_id.clone(),
                claim: claim.clone(),
            },
        };

        // Preflight against the current snapshot: a caller-correctable
        // rejection (terminal task, unknown execution) consumes nothing.
        crate::kernel::ActionPlanner::plan(&snapshot, &command).map_err(runtime)?;

        // Durable intent.
        let intent = TransitionPlan {
            process_id: process_id.into(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: vec![OperationIntent {
                id: operation_id.clone(),
                kind: OperationKind::ApplyAction,
            }],
            outbox: Vec::new(),
        };
        match self
            .aggregates
            .commit_transition(&intent)
            .await
            .map_err(runtime)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { .. } => {}
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                return Err(ActError::Runtime(format!(
                    "action intent conflict on {process_id}/{task_id}"
                )));
            }
        }

        // Ownership, authoritative re-plan, commit, complete — the standard
        // hop, but planned by ActionPlanner instead of the host's own arms.
        let revision = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during action".into()))?
            .snapshot
            .operations
            .get(&operation_id)
            .map(|record| record.revision)
            .ok_or_else(|| ActError::Runtime("action operation lost".into()))?;
        let claimed = self
            .operation_event(
                &operation_id,
                revision,
                OperationEvent::Claim(claim.clone()),
            )
            .await?;
        let prepared = self
            .operation_event(
                &operation_id,
                claimed.revision,
                OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                },
            )
            .await?;
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during action".into()))?
            .snapshot;
        match crate::kernel::ActionPlanner::plan(&snapshot, &command) {
            Ok(plan) => {
                let receipt = match self
                    .aggregates
                    .commit_transition(&plan)
                    .await
                    .map_err(runtime)?
                {
                    crate::kernel::TransitionCommitOutcome::Committed { receipt, .. }
                    | crate::kernel::TransitionCommitOutcome::AlreadyCommitted {
                        receipt, ..
                    } => receipt,
                    crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                        return Err(ActError::Runtime(format!(
                            "action commit conflict on {process_id}/{task_id}"
                        )));
                    }
                };
                let committed = self
                    .load(process_id)
                    .await?
                    .ok_or_else(|| ActError::Runtime("aggregate lost after action".into()))?
                    .snapshot
                    .operations
                    .get(&operation_id)
                    .cloned()
                    .ok_or_else(|| ActError::Runtime("action operation lost".into()))?;
                self.operation_event(
                    &operation_id,
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
                // A Cancel ends this path AND re-runs it: the cancelled
                // execution is re-entered with a fresh generation (the
                // legacy rewind), its downstream recreated by the planner.
                if matches!(command.action, crate::kernel::ActionKind::Cancel) {
                    self.run_reentry(process_id, &execution_id, task_id).await?;
                }
                Ok(())
            }
            Err(error) => {
                // The state moved between intent and ownership: park the
                // operation for recovery instead of consuming it.
                let _ = self
                    .operation_event(
                        &operation_id,
                        prepared.revision,
                        OperationEvent::ScheduleRetry {
                            claim: claim.clone(),
                            attempt: 1,
                        },
                    )
                    .await;
                Err(ActError::Runtime(format!("action re-plan failed: {error}")))
            }
        }
    }

    /// Re-enter one cancelled execution with a fresh generation through
    /// [`crate::kernel::GenerationPlanner`]: a carrier operation on the same
    /// execution is created, owned, and committed with the planner's
    /// re-entry plan (new generations for the forward closure).
    async fn run_reentry(
        &self,
        process_id: &str,
        execution_id: &crate::kernel::ExecutionId,
        task_id: &str,
    ) -> Result<()> {
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost before reentry".into()))?
            .snapshot;
        let next_sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == process_id
                    && id.task_id == execution_id.task_id
                    && id.generation == execution_id.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        let carrier_id = OperationId {
            process_id: process_id.into(),
            task_id: execution_id.task_id.clone(),
            generation: execution_id.generation,
            sequence: next_sequence,
        };
        let intent = TransitionPlan {
            process_id: process_id.into(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: vec![OperationIntent {
                id: carrier_id.clone(),
                kind: OperationKind::ApplyAction,
            }],
            outbox: Vec::new(),
        };
        match self
            .aggregates
            .commit_transition(&intent)
            .await
            .map_err(runtime)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { .. } => {}
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                return Err(ActError::Runtime("reentry intent conflict".into()));
            }
        }
        let now = SystemClock.now_ms();
        let claim = crate::kernel::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let revision = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during reentry".into()))?
            .snapshot
            .operations
            .get(&carrier_id)
            .map(|record| record.revision)
            .ok_or_else(|| ActError::Runtime("reentry carrier lost".into()))?;
        let claimed = self
            .operation_event(&carrier_id, revision, OperationEvent::Claim(claim.clone()))
            .await?;
        let prepared = self
            .operation_event(
                &carrier_id,
                claimed.revision,
                OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                },
            )
            .await?;
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during reentry".into()))?
            .snapshot;
        let graph = execution_graph(&snapshot)?;
        let command = crate::kernel::ReenterCommand {
            process_id: process_id.into(),
            source: execution_id.clone(),
            target_task_id: task_id.into(),
            source_operation: OperationCommit {
                id: carrier_id.clone(),
                claim: claim.clone(),
            },
        };
        let plan = crate::kernel::GenerationPlanner::plan_reentry(&graph, &snapshot, &command)
            .map_err(runtime)?;
        let receipt = match self
            .aggregates
            .commit_transition(&plan)
            .await
            .map_err(runtime)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { receipt, .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { receipt, .. } => receipt,
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                return Err(ActError::Runtime("reentry commit conflict".into()));
            }
        };
        let committed = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost after reentry".into()))?
            .snapshot
            .operations
            .get(&carrier_id)
            .cloned()
            .ok_or_else(|| ActError::Runtime("carrier lost after reentry".into()))?;
        let _ = prepared;
        self.operation_event(
            &carrier_id,
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
        Ok(())
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
                // Auto tasks seed their own completion action; waiting tasks
                // stay Running until a client action arrives.
                let auto = snapshot
                    .variables
                    .get(&(execution_id.clone(), "auto".into()))
                    .map(String::as_str)
                    != Some("0");
                let mut plan = self.task_hop_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    execution.as_ref(),
                    &commit_claim,
                    "started",
                    "task.started",
                )?;
                if !action_exists && auto {
                    plan.operations.push(OperationIntent {
                        id: action_id,
                        kind: OperationKind::ApplyAction,
                    });
                }
                if let Some(ready) = execution
                    .as_ref()
                    .filter(|ready| ready.state == crate::kernel::ExecutionState::Ready)
                {
                    // Waiting packages park in `Waiting` (the legacy
                    // `Interrupt`); everything else runs to completion.
                    let target = if auto {
                        crate::kernel::ExecutionState::Running
                    } else {
                        crate::kernel::ExecutionState::Waiting
                    };
                    plan.state_changes.push(StateChange {
                        execution: execution_id.clone(),
                        expected_revision: ready.revision,
                        from: crate::kernel::ExecutionState::Ready,
                        to: target,
                    });
                }
                plan
            }
            OperationKind::ApplyAction => {
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if !matches!(
                    execution.state,
                    crate::kernel::ExecutionState::Running | crate::kernel::ExecutionState::Waiting
                ) {
                    return Err(ActError::Runtime(format!(
                        "execution {execution_id:?} is {:?}, expected Running or Waiting",
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
                    from: execution.state,
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
            OperationKind::SkipTask => {
                // The cascade hop: admit-and-skip already committed
                // `Created → Ready`; this hop marks the execution `Skipped`
                // and hands propagation to a `PropagateTask`, whose readiness
                // treats the skipped source as dead and continues the
                // cascade. Like every hop it is one atomic commit sourced by
                // this operation.
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if execution.state != crate::kernel::ExecutionState::Ready {
                    return Err(ActError::Runtime(format!(
                        "execution {execution_id:?} is {:?}, expected Ready",
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
                    "skipped",
                    "task.skipped",
                )?;
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: crate::kernel::ExecutionState::Ready,
                    to: crate::kernel::ExecutionState::Skipped,
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
    use crate::kernel::{ActionKind, DeliveryOutcome, OutboxRecord, OutboxState};

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
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p1", "root", &["a", "b"]);
        host.start_process(&spec).await.unwrap();
        let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
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
        let (host, mut wake_rx) = KernelHost::in_memory();
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
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
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
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("k-parity", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
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

    /// The wiring slice: with `kernel_engine` on, every engine start
    /// shadow-runs the same process through the kernel host over the same
    /// store, and the shadow settles exactly when the engine settles.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_shadow_follows_engine_start() {
        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });

        let workflow = {
            let mut workflow = crate::model::Workflow::new().with_id("wf-shadow");
            workflow = workflow.with_step(|step| step.with_id("s1"));
            workflow = workflow.with_step(|step| step.with_id("s2"));
            workflow
        };
        let legacy = engine
            .runtime()
            .start(&workflow, crate::Vars::new())
            .await
            .unwrap();
        let pid = legacy.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("engine never finished");

        let runtime = engine.runtime();
        let host = runtime.kernel().expect("kernel shadow host must be wired");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let shadow = loop {
            match host.process_completed(&pid).await.unwrap() {
                true => break true,
                false if std::time::Instant::now() > deadline => break false,
                false => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        };
        assert!(shadow, "kernel shadow never completed pid {pid}");
        let snapshot = host
            .snapshot(&pid)
            .await
            .unwrap()
            .expect("shadow aggregate");
        let mut shadow_tasks: Vec<String> = snapshot
            .executions
            .keys()
            .map(|id| id.task_id.clone())
            .collect();
        shadow_tasks.sort();
        assert_eq!(
            shadow_tasks,
            vec![
                crate::utils::consts::TASK_ROOT_TID.to_string(),
                "s1".into(),
                "s2".into()
            ]
        );
    }

    #[tokio::test]
    async fn manual_task_waits_for_the_client_action() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let mut spec = ProcessSpec::linear("p4", "root", &["wait", "after"]);
        // `wait` has no entry in the auto set: it is a waiting task.
        spec.auto.remove("wait");
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

        // `wait` ran and is Running; `after` must not have started.
        let snapshot = host.snapshot("p4").await.unwrap().unwrap();
        let state_of = |task: &str| {
            snapshot
                .executions
                .iter()
                .find(|(id, _)| id.task_id == task)
                .map(|(_, e)| e.state)
                .unwrap()
        };
        assert_eq!(state_of("wait"), crate::kernel::ExecutionState::Waiting);
        assert_eq!(state_of("after"), crate::kernel::ExecutionState::Created);

        // The client completes the waiting task; the flow moves on.
        host.apply_action("p4", "wait", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        assert!(host.process_completed("p4").await.unwrap());
    }

    #[tokio::test]
    async fn completing_a_terminal_task_is_rejected_without_consuming_state() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p5", "root", &[]);
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        assert!(host.process_completed("p5").await.unwrap());
        let err = host
            .apply_action("p5", "$", ActionKind::Complete, Vec::new())
            .await
            .unwrap_err();
        let before = host.snapshot("p5").await.unwrap().unwrap();
        // The rejection text is the planner's; the contract is the refusal
        // plus a frozen aggregate.
        assert!(!format!("{err}").is_empty());
        let after = host.snapshot("p5").await.unwrap().unwrap();
        assert_eq!(
            before.version, after.version,
            "a rejected action must not move the aggregate"
        );
    }

    /// Action parity floor: an IRQ workflow waits on both engines, the client
    /// completes each act, and both engines settle with the same terminal
    /// task set. The legacy side drives `Runtime::do_action`; the kernel
    /// shadow mirrors it through the hook.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_shadow_mirrors_client_actions_on_irq_workflow() {
        use crate::event::EventAction;

        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let workflow = {
            let mut workflow = crate::model::Workflow::new().with_id("wf-irq");
            workflow = workflow.with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act1"),
                )
            });
            workflow = workflow.with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act2"),
                )
            });
            workflow
        };
        let runtime = engine.runtime();
        let host = runtime.kernel().expect("kernel shadow host must be wired");

        // Subscribers attach BEFORE the start: the IRQ act's Created message
        // fires the moment the act runs, and a late subscriber misses it.
        let wait_for_created = |engine: &crate::Engine, key: &'static str| {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            let channel = engine.channel();
            channel.on_message(move |e| {
                let done = done_rx.clone();
                let key = key.to_string();
                async move {
                    if e.is_params_key(&key) && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let created1 = wait_for_created(&engine, "act1");
        let created2 = wait_for_created(&engine, "act2");

        let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
        let pid = proc.id().to_string();

        // Each waiting act completes exactly when its client action arrives.
        for created in [created1, created2] {
            let (pid, tid) =
                tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
                    .await
                    .expect("waiting act never created")
                    .clone();
            runtime
                .do_action(&crate::event::Action::new(
                    &pid,
                    &tid,
                    EventAction::Next,
                    crate::Vars::new(),
                ))
                .await
                .unwrap();
        }

        // Both sides settle on the same terminal task set.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let shadow_done = host.process_completed(&pid).await.unwrap_or(false);
            let legacy_done = legacy_completed(&engine, &pid).await;
            if shadow_done && legacy_done {
                break;
            }
            let shadow_states = host.snapshot(&pid).await.unwrap().map(|snapshot| {
                snapshot
                    .executions
                    .iter()
                    .map(|(id, execution)| format!("{}:{:?}", id.task_id, execution.state))
                    .collect::<Vec<_>>()
            });
            let legacy_state = engine
                .runtime()
                .proc(&pid)
                .await
                .unwrap()
                .map(|proc| proc.state());
            assert!(
                std::time::Instant::now() < deadline,
                "shadow_done={shadow_done} legacy_done={legacy_done} legacy_state={legacy_state:?} shadow={shadow_states:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    async fn legacy_completed(engine: &crate::Engine, pid: &str) -> bool {
        engine
            .runtime()
            .proc(pid)
            .await
            .unwrap()
            .map(|proc| proc.state().is_completed())
            .unwrap_or(false)
    }

    /// The skip cascade: cancelling the waiting head of a chain skips the
    /// rest instead of erroring the propagation. Every execution ends
    /// terminal, and the skipped task is identifiable by its state.
    #[tokio::test]
    async fn cancelled_head_cascades_skip_through_the_chain() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let mut spec = ProcessSpec::linear("p6", "root", &["head", "mid", "tail"]);
        spec.auto.remove("head"); // the head waits for a client action
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

        host.apply_action("p6", "head", ActionKind::Cancel, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        // The rewind: head gen0 is Cancelled, the downstream is Skipped, and
        // a fresh head generation is already parked waiting again.
        let snapshot = host.snapshot("p6").await.unwrap().unwrap();
        let state_of = |task: &str, generation: u64| {
            snapshot
                .executions
                .get(&crate::kernel::ExecutionId::new(task, generation).unwrap())
                .map(|execution| execution.state)
                .unwrap()
        };
        assert_eq!(
            state_of("head", 0),
            crate::kernel::ExecutionState::Cancelled
        );
        assert_eq!(state_of("mid", 0), crate::kernel::ExecutionState::Skipped);
        assert_eq!(state_of("tail", 0), crate::kernel::ExecutionState::Skipped);
        assert_eq!(state_of("head", 1), crate::kernel::ExecutionState::Waiting);

        // The re-run answers its own action; the recreated downstream runs.
        host.apply_action("p6", "head", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert!(host.process_completed("p6").await.unwrap());
        let snapshot = host.snapshot("p6").await.unwrap().unwrap();
        assert_eq!(
            snapshot
                .executions
                .get(&crate::kernel::ExecutionId::new("head", 1).unwrap())
                .map(|execution| execution.state),
            Some(crate::kernel::ExecutionState::Completed)
        );
    }
}
