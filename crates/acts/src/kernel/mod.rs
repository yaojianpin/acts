//! Execution kernel.
//!
//! This module is deliberately independent from queues, caches, resident
//! process objects and storage implementations.  It defines the durable
//! operation lifecycle that all execution entry points converge on.
//
// The kernel's typed error enums carry rich, caller-actionable payloads
// (operation ids, revisions, conflicting tokens) by value — that is the
// deliberate contract of a deterministic core. Boxing them is deferred to
// the shell-integration phase; until then the size lint is scoped here and
// nowhere else.
#![allow(clippy::result_large_err)]
mod application;
#[cfg(test)]
mod architecture;
mod async_document_store;
mod branch;
mod branch_application;
mod clock;
mod command;
mod commit;
#[cfg(test)]
mod coverage;
mod document;
mod document_store;
mod envelope;
mod execution_admission;
mod execution_graph;
mod failure_recovery;
mod generation;
mod generation_reference;
mod model_graph;
mod operation;
mod operation_executor;
mod operation_phase;
mod outbox;
mod outbox_codec;
mod outbox_dispatch;
mod outbox_index;
mod outbox_repository;
mod outbox_worker;
mod process_recovery;
mod propagation;
mod recovery;
mod recovery_entry;
mod recovery_format;
mod recovery_index;
mod replay_policy;
mod snapshot_codec;
mod transition_plan;

pub(crate) use application::{OperationWakeSink, WakeError};
pub(crate) use async_document_store::{
    AsyncAggregateRepository, AsyncDocumentStore, AsyncRecoveryRepository, KvDocumentStore,
    OperationCommitOutcome, RecoveryMigrationBatch, RecoveryMigrationState,
    TransitionCommitOutcome,
};
pub(crate) use clock::{Clock, FixedClock, SystemClock};
pub(crate) use commit::{
    AggregateRepository, CommitError, CommitReceipt, OperationRepository, RecoveryRepository,
};
pub(crate) use document_store::{
    AggregateDocumentRepository, AtomicDocumentStore, LoadedAggregate, MemoryDocumentStore,
};
pub(crate) use failure_recovery::FailureRecovery;
pub(crate) use outbox_dispatch::{DeliveryOutcome, OutboxTransport};
pub(crate) use outbox_worker::{DispatchBatchConfig, DispatchBatchReport, dispatch_batch};
pub(crate) use propagation::{JoinPolicy, PropagateCommand, PropagationPlanner};
pub(crate) use recovery::{RecoveryDisposition, RecoveryPlanner, RecoveryReport, RecoveryService};

#[allow(unused_imports)]
pub(crate) use outbox::{
    DeliveryClaim, OutboxError, OutboxEvent, OutboxId, OutboxRecord, OutboxState,
};
pub(crate) use outbox_index::{OutboxIndexCodec, OutboxIndexEntry, OutboxIndexState};
pub(crate) use outbox_repository::{OutboxCursor, OutboxPage};

pub(crate) use process_recovery::{
    DurableProcessState, ProcessRecoveryDisposition, ProcessRecoveryPolicy,
};
pub(crate) use replay_policy::{
    BootTaskObservation, BootTaskPlan, BootTaskPlanningPolicy, DispatchConfirmationPolicy,
    DurableRecoveryFact, DurableReplayPolicy, RecoveredAction, RecoveryAttemptAssessment,
    RecoveryAttemptPolicy, RecoveryCandidate, RecoveryCloseReason, RecoveryFactError,
    RecoveryObservation, RecoveryOperationFact, RecoveryPlan, ReplayCommand, ReplayOperationKind,
    ReplayPlacement, ResidentRecoveryObservation, ResidentRecoveryPlan, TerminalReplayPolicy,
};

pub(crate) use operation_phase::{OperationPhase, PhaseTransition};

pub(crate) use operation::{
    ClaimToken, CommitToken, OperationEvent, OperationId, OperationKind, OperationRecord,
    OperationState,
};

pub(crate) use model_graph::{
    ChildRole, ModelGraph, ModelGraphBuilder, NodeDefinition, NodeId, NodeKind,
};

pub(crate) use execution_admission::{
    AdmissionDecision, AdmissionIntent, ExecutionSlotKind, ExistingGeneration,
    GenerationAdmissionPolicy, SlotObservation,
};

pub(crate) use generation::{GenerationError, GenerationPlanner, ReenterCommand};
pub(crate) use generation_reference::{GenerationReference, HandoffIdentity, RecoveryGeneration};

pub(crate) use execution_graph::{
    ExecutionGraph, ExecutionGraphBuilder, ExecutionId, ExecutionNode, ExecutionState,
};

pub(crate) use transition_plan::{
    AggregateVersion, ExecutionCreate, OperationCommit, OperationIntent, OutboxIntent, StateChange,
    TransitionPlan, VariableWrite,
};

#[cfg(test)]
pub(crate) use commit::MemoryAggregateRepository;
pub(crate) use commit::{AggregateSnapshot, ExecutionSnapshot};

pub(crate) use command::{ActionKind, ActionPlanner, ApplyActionCommand, CommandError};

pub(crate) use branch::PredicateValue;
