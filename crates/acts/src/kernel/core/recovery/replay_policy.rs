//! Pure durable-operation replay policy.
//!
//! Storage encodings are translated at the scheduler boundary. The kernel owns
//! whether a durable operation class and placement require replay.

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum ReplayOperationKind {
    Execute,
    Propagate,
    Action,
    Error,
    Abort,
}

impl ReplayOperationKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Propagate => "propagate",
            Self::Action => "action",
            Self::Error => "error",
            Self::Abort => "abort",
        }
    }
}

/// Kernel-owned liveness assessment for repeated recovery attempts.
/// The scheduler reports attempts; the kernel decides when non-convergence
/// becomes operationally significant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryAttemptAssessment {
    Continue,
    WarnNonConverging,
}

/// Kernel-owned work budget for durable dispatch confirmation reconciliation.
/// The scheduler executes the work; the kernel limits how much one recovery
/// pass may consume so confirmation I/O cannot starve ordinary replay.
pub(crate) struct DispatchConfirmationPolicy;

impl DispatchConfirmationPolicy {
    pub(crate) const BATCH_LIMIT: usize = 32;
    pub(crate) const LEASE: std::time::Duration = std::time::Duration::from_secs(30);
    const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(300);
    const NON_CONVERGING_ATTEMPTS: u32 = 8;
    const NON_CONVERGING_AGE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

    pub(crate) const fn is_non_converging(attempts: u32, age: std::time::Duration) -> bool {
        attempts >= Self::NON_CONVERGING_ATTEMPTS
            || age.as_secs() >= Self::NON_CONVERGING_AGE.as_secs()
    }

    pub(crate) const fn retry_delay(attempts: u32) -> std::time::Duration {
        let raw_shift = attempts.saturating_sub(1);
        let shift = if raw_shift > 8 { 8 } else { raw_shift };
        let seconds = 1_u64 << shift;
        let delay = std::time::Duration::from_secs(seconds);
        if delay.as_secs() > Self::MAX_RETRY_DELAY.as_secs() {
            Self::MAX_RETRY_DELAY
        } else {
            delay
        }
    }
}

pub(crate) struct RecoveryAttemptPolicy;

impl RecoveryAttemptPolicy {
    const PROPAGATION_WARNING_ATTEMPT: u32 = 4;

    pub(crate) const fn assess(
        kind: ReplayOperationKind,
        attempts: u32,
    ) -> RecoveryAttemptAssessment {
        if matches!(kind, ReplayOperationKind::Propagate)
            && attempts == Self::PROPAGATION_WARNING_ATTEMPT
        {
            RecoveryAttemptAssessment::WarnNonConverging
        } else {
            RecoveryAttemptAssessment::Continue
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplayPlacement {
    Pending,
    Dispatched,
    Overflow,
    Done,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplayCommand {
    Execute,
    Propagate,
    Error,
    Abort,
}

impl ReplayCommand {
    pub(crate) const fn is_propagation(self) -> bool {
        matches!(self, Self::Propagate | Self::Error | Self::Abort)
    }

    pub(crate) const fn operation(self) -> ReplayOperationKind {
        match self {
            Self::Execute => ReplayOperationKind::Execute,
            Self::Propagate => ReplayOperationKind::Propagate,
            Self::Error => ReplayOperationKind::Error,
            Self::Abort => ReplayOperationKind::Abort,
        }
    }
}

/// Runtime facts needed to recover a task whose process is already resident
/// after boot. Durable-operation ownership is explicit so the boot path cannot
/// race durable replay for the same task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BootTaskObservation {
    pub execution_state: crate::kernel::ExecutionState,
    pub has_children: bool,
    pub durable_operation_owned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BootTaskPlan {
    LeaveParked,
    Readmit,
    ResetThenReadmit,
    AwaitDurableRecovery,
}

pub(crate) struct BootTaskPlanningPolicy;

impl BootTaskPlanningPolicy {
    pub(crate) const fn plan(observation: BootTaskObservation) -> BootTaskPlan {
        if observation.durable_operation_owned {
            return BootTaskPlan::AwaitDurableRecovery;
        }
        if observation.has_children {
            return BootTaskPlan::LeaveParked;
        }
        match observation.execution_state {
            crate::kernel::ExecutionState::Created | crate::kernel::ExecutionState::Ready => {
                BootTaskPlan::Readmit
            }
            crate::kernel::ExecutionState::Running => BootTaskPlan::ResetThenReadmit,
            _ => BootTaskPlan::LeaveParked,
        }
    }
}

/// Why recovery may consume an open durable operation without replaying it.
/// Carrying the reason through the writer protocol prevents an unclassified
/// terminal mutation and gives diagnostics a stable semantic vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum RecoveryCloseReason {
    AlreadyClosedPlacement,
    ProcessMissing,
    TaskMissing,
    EffectAlreadyDurable,
    SupersededGeneration,
    TerminalTask,
}

impl RecoveryCloseReason {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::AlreadyClosedPlacement => "already-closed-placement",
            Self::ProcessMissing => "process-missing",
            Self::TaskMissing => "task-missing",
            Self::EffectAlreadyDurable => "effect-already-durable",
            Self::SupersededGeneration => "superseded-generation",
            Self::TerminalTask => "terminal-task",
        }
    }

    /// Whether disappearance of the exact durable row is explained by this proof.
    /// Aggregate/task absence and an already-closed placement can legitimately
    /// race with cleanup; effect and lifecycle proofs cannot.
    pub(crate) const fn explains_missing(self) -> bool {
        matches!(
            self,
            Self::AlreadyClosedPlacement | Self::ProcessMissing | Self::TaskMissing
        )
    }

    /// Whether observing a newer generation is the expected resolution of the
    /// close proof rather than an unexplained recovery race.
    pub(crate) const fn explains_stale_generation(self) -> bool {
        matches!(self, Self::SupersededGeneration)
    }

    /// Reject semantically impossible close proofs before they reach storage.
    /// Infrastructure absence can retire any kind; effect/generation/task
    /// proofs are intentionally narrower.
    pub(crate) const fn permits(self, kind: ReplayOperationKind) -> bool {
        match self {
            Self::AlreadyClosedPlacement | Self::ProcessMissing | Self::TaskMissing => true,
            Self::EffectAlreadyDurable => {
                matches!(
                    kind,
                    ReplayOperationKind::Execute | ReplayOperationKind::Action
                )
            }
            Self::SupersededGeneration => {
                matches!(
                    kind,
                    ReplayOperationKind::Propagate
                        | ReplayOperationKind::Error
                        | ReplayOperationKind::Abort
                )
            }
            Self::TerminalTask => {
                matches!(
                    kind,
                    ReplayOperationKind::Propagate
                        | ReplayOperationKind::Error
                        | ReplayOperationKind::Abort
                )
            }
        }
    }
}

pub(crate) struct DurableReplayPolicy;

impl DurableReplayPolicy {
    /// Effectful kinds replay from every *open* placement. `Dispatched` and
    /// `Overflow` are open: the in-memory handoff is not itself durable
    /// (see `store::data::OpStatus::Dispatched`), so after a crash no worker
    /// owns the row and closing it would lose the operation. The double-run
    /// guards live above and below this table: `RecoveryPlanningPolicy`
    /// awaits confirmation while a live owner exists, and the resident
    /// inspection layer closes on terminal state, durable effects, and
    /// superseded generations.
    pub(crate) const fn decide(
        kind: ReplayOperationKind,
        placement: ReplayPlacement,
    ) -> Option<ReplayCommand> {
        match (kind, placement) {
            (ReplayOperationKind::Propagate, _) => Some(ReplayCommand::Propagate),
            (_, ReplayPlacement::Done) => None,
            (ReplayOperationKind::Execute, _) => Some(ReplayCommand::Execute),
            (ReplayOperationKind::Error, _) => Some(ReplayCommand::Error),
            (ReplayOperationKind::Abort, _) => Some(ReplayCommand::Abort),
            // Actions never replay by scan; the action recovery path
            // inspects the durable payload and resident state instead.
            (ReplayOperationKind::Action, _) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_close_reasons_explain_only_their_expected_absence_races() {
        for reason in [
            RecoveryCloseReason::AlreadyClosedPlacement,
            RecoveryCloseReason::ProcessMissing,
            RecoveryCloseReason::TaskMissing,
        ] {
            assert!(reason.explains_missing());
        }
        for reason in [
            RecoveryCloseReason::EffectAlreadyDurable,
            RecoveryCloseReason::SupersededGeneration,
            RecoveryCloseReason::TerminalTask,
        ] {
            assert!(!reason.explains_missing());
        }
        assert!(RecoveryCloseReason::SupersededGeneration.explains_stale_generation());
        assert!(!RecoveryCloseReason::TerminalTask.explains_stale_generation());
    }

    #[test]
    fn recovery_close_reasons_reject_semantically_invalid_operation_kinds() {
        assert!(RecoveryCloseReason::EffectAlreadyDurable.permits(ReplayOperationKind::Execute));
        assert!(RecoveryCloseReason::EffectAlreadyDurable.permits(ReplayOperationKind::Action));
        assert!(!RecoveryCloseReason::EffectAlreadyDurable.permits(ReplayOperationKind::Propagate));

        for kind in [
            ReplayOperationKind::Propagate,
            ReplayOperationKind::Error,
            ReplayOperationKind::Abort,
        ] {
            assert!(RecoveryCloseReason::SupersededGeneration.permits(kind));
            assert!(RecoveryCloseReason::TerminalTask.permits(kind));
        }
        assert!(!RecoveryCloseReason::SupersededGeneration.permits(ReplayOperationKind::Execute));
        assert!(!RecoveryCloseReason::TerminalTask.permits(ReplayOperationKind::Action));

        for kind in [
            ReplayOperationKind::Execute,
            ReplayOperationKind::Propagate,
            ReplayOperationKind::Action,
            ReplayOperationKind::Error,
            ReplayOperationKind::Abort,
        ] {
            assert!(RecoveryCloseReason::ProcessMissing.permits(kind));
            assert!(RecoveryCloseReason::TaskMissing.permits(kind));
        }
    }

    #[test]
    fn propagation_is_replayed_from_every_open_or_closed_placement() {
        for placement in [
            ReplayPlacement::Pending,
            ReplayPlacement::Dispatched,
            ReplayPlacement::Overflow,
            ReplayPlacement::Done,
        ] {
            assert_eq!(
                DurableReplayPolicy::decide(ReplayOperationKind::Propagate, placement),
                Some(ReplayCommand::Propagate)
            );
        }
    }

    #[test]
    fn effectful_hops_replay_from_every_open_placement() {
        for placement in [
            ReplayPlacement::Pending,
            ReplayPlacement::Overflow,
            ReplayPlacement::Dispatched,
        ] {
            assert_eq!(
                DurableReplayPolicy::decide(ReplayOperationKind::Execute, placement),
                Some(ReplayCommand::Execute)
            );
            assert_eq!(
                DurableReplayPolicy::decide(ReplayOperationKind::Error, placement),
                Some(ReplayCommand::Error)
            );
            assert_eq!(
                DurableReplayPolicy::decide(ReplayOperationKind::Abort, placement),
                Some(ReplayCommand::Abort)
            );
        }
        // A Done row's effect is durable by definition; the planning layer
        // closes it before this table runs.
        assert_eq!(
            DurableReplayPolicy::decide(ReplayOperationKind::Execute, ReplayPlacement::Done),
            None
        );
        // Actions never replay by scan.
        assert_eq!(
            DurableReplayPolicy::decide(ReplayOperationKind::Action, ReplayPlacement::Pending),
            None
        );
    }
}

/// Decides whether a terminal resident task proves that a recovered operation
/// no longer requires replay. The policy is deliberately expressed only in
/// kernel vocabulary; adapters supply durable-effect evidence separately.
pub(crate) struct TerminalReplayPolicy;

impl TerminalReplayPolicy {
    pub(crate) fn close_reason(
        state: crate::kernel::ExecutionState,
        is_propagation: bool,
        effect_is_durable: bool,
    ) -> Option<RecoveryCloseReason> {
        if !state.is_terminal() {
            return None;
        }
        if is_propagation && effect_is_durable {
            return None;
        }
        Some(RecoveryCloseReason::TerminalTask)
    }

    /// State-guarded actions cannot have remained pending once their target is
    /// terminal. State-independent actions still require idempotent replay.
    pub(crate) fn action_effect_is_durable(
        state: crate::kernel::ExecutionState,
        state_independent_action: bool,
    ) -> bool {
        state.is_terminal() && !state_independent_action
    }
}

#[cfg(test)]
mod terminal_replay_tests {
    use super::*;
    use crate::kernel::ExecutionState;

    #[test]
    fn terminal_replay_policy_preserves_durable_propagation_and_idempotent_actions() {
        assert_eq!(
            TerminalReplayPolicy::close_reason(ExecutionState::Completed, false, false),
            Some(RecoveryCloseReason::TerminalTask)
        );
        assert_eq!(
            TerminalReplayPolicy::close_reason(ExecutionState::Completed, true, true),
            None
        );
        assert_eq!(
            TerminalReplayPolicy::close_reason(ExecutionState::Running, false, false),
            None
        );
        assert!(TerminalReplayPolicy::action_effect_is_durable(
            ExecutionState::Completed,
            false
        ));
        assert!(!TerminalReplayPolicy::action_effect_is_durable(
            ExecutionState::Completed,
            true
        ));
    }
}

/// Complete kernel interpretation of one durable recovery record.
/// Adapters decode storage vocabulary once; orchestration consumes this value
/// without independently recombining kind, placement, phase, or generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DurableRecoveryFact {
    kind: ReplayOperationKind,
    placement: ReplayPlacement,
    phase: crate::kernel::OperationPhase,
    generation: crate::kernel::RecoveryGeneration,
}

impl DurableRecoveryFact {
    pub(crate) fn new(
        kind: ReplayOperationKind,
        placement: ReplayPlacement,
        phase: crate::kernel::OperationPhase,
        source_generation: i64,
        target_task_id: Option<String>,
    ) -> Self {
        Self {
            kind,
            placement,
            phase,
            generation: crate::kernel::RecoveryGeneration::from_persisted(
                source_generation,
                target_task_id,
            ),
        }
    }

    pub(crate) const fn kind(&self) -> ReplayOperationKind {
        self.kind
    }

    pub(crate) fn replay_command(&self) -> Option<ReplayCommand> {
        DurableReplayPolicy::decide(self.kind, self.placement)
    }

    pub(crate) const fn effect_is_durable(&self) -> bool {
        self.phase.effect_is_durable()
    }

    pub(crate) const fn source_generation(&self) -> i64 {
        self.generation.persisted()
    }

    pub(crate) const fn resolve_generation(&self, resident_generation: i64) -> i64 {
        self.generation.resolve(resident_generation)
    }

    pub(crate) fn describes_handoff(
        &self,
        resident_generation: i64,
        resident_parent_task_id: Option<&str>,
    ) -> bool {
        self.generation
            .describes_handoff(resident_generation, resident_parent_task_id)
    }
}

// -----------------------------------------------------------------------------
// Durable row decoding
// -----------------------------------------------------------------------------
// Store representations are translated once, beside the replay policy that
// consumes them. Runtime code receives typed recovery candidates and cannot
// reinterpret persistence strings or construct partial recovery facts.

// Pure recovery classification.
//
// This module knows durable facts and task-state values, but owns no Store,
// Process, Queue, Runtime, or resident mutation. Recovery orchestration must
// obtain a decision here before performing I/O or changing runtime state.

use crate::{
    Vars, event::EventAction, kernel::ExecutionState, runtime_state::OperationIdentity, store::data,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryFactError {
    MissingProcessId,
    MissingTaskId,
    SourceTaskMismatch,
    OperationType,
    OperationStatus,
    OperationPhase,
    ActionEvent,
    ActionOptions,
    MissingActionPayload,
    UnexpectedActionPayload,
}

impl RecoveryFactError {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MissingProcessId => "process id",
            Self::MissingTaskId => "task id",
            Self::SourceTaskMismatch => "source task identity",
            Self::OperationType => "operation type",
            Self::OperationStatus => "operation status",
            Self::OperationPhase => "operation phase",
            Self::ActionEvent => "action event",
            Self::ActionOptions => "action options",
            Self::MissingActionPayload => "action payload",
            Self::UnexpectedActionPayload => "unexpected action payload",
        }
    }
}

#[derive(Clone)]
pub(crate) struct RecoveredAction {
    pub event: EventAction,
    pub options: Vars,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryObservation {
    pub confirmation_owned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryPlan {
    AwaitConfirmation,
    IgnoreClosed,
    Close(RecoveryCloseReason),
    InspectResident(ReplayCommand),
    InspectAction,
}

/// Stable resident facts observed after a durable recovery candidate has been
/// admitted for inspection. Runtime owns object access; the kernel owns every
/// interpretation of generation, handoff identity, and terminal state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResidentRecoveryObservation<'a> {
    pub execution_state: ExecutionState,
    pub propagation_done: bool,
    pub generation: i64,
    pub parent_task_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResidentRecoveryPlan {
    Close(RecoveryCloseReason),
    Dispatch {
        command: ReplayCommand,
        generation: i64,
    },
}

struct ResidentRecoveryPlanningPolicy;

impl ResidentRecoveryPlanningPolicy {
    fn plan(
        fact: &DurableRecoveryFact,
        command: ReplayCommand,
        observation: ResidentRecoveryObservation<'_>,
    ) -> ResidentRecoveryPlan {
        if command == ReplayCommand::Propagate
            && !fact.describes_handoff(observation.generation, observation.parent_task_id)
        {
            return ResidentRecoveryPlan::Close(RecoveryCloseReason::SupersededGeneration);
        }

        if let Some(reason) = TerminalReplayPolicy::close_reason(
            observation.execution_state,
            observation.propagation_done,
            fact.effect_is_durable(),
        ) {
            return ResidentRecoveryPlan::Close(reason);
        }

        ResidentRecoveryPlan::Dispatch {
            command,
            generation: fact.resolve_generation(observation.generation),
        }
    }
}

pub(crate) struct RecoveryPlanningPolicy;

impl RecoveryPlanningPolicy {
    pub(crate) fn plan(
        fact: &DurableRecoveryFact,
        observation: RecoveryObservation,
    ) -> RecoveryPlan {
        if observation.confirmation_owned {
            return RecoveryPlan::AwaitConfirmation;
        }
        if fact.placement == ReplayPlacement::Done {
            return RecoveryPlan::IgnoreClosed;
        }
        if fact.effect_is_durable() {
            return RecoveryPlan::Close(RecoveryCloseReason::EffectAlreadyDurable);
        }
        match fact.replay_command() {
            Some(command) => RecoveryPlan::InspectResident(command),
            None if fact.kind() == ReplayOperationKind::Action => RecoveryPlan::InspectAction,
            None => RecoveryPlan::IgnoreClosed,
        }
    }
}

pub(crate) struct RecoveryCandidate {
    fact: RecoveryOperationFact,
    identity: OperationIdentity,
}

impl RecoveryCandidate {
    pub(crate) fn decode(row: &data::Op) -> Result<Self, RecoveryFactError> {
        let fact = RecoveryOperationFact::from_row(row)?;
        let identity = OperationIdentity::new(
            fact.pid(),
            fact.tid(),
            fact.replay_kind(),
            fact.source_version(),
        );
        Ok(Self { fact, identity })
    }

    pub(crate) const fn fact(&self) -> &RecoveryOperationFact {
        &self.fact
    }

    pub(crate) const fn identity(&self) -> &OperationIdentity {
        &self.identity
    }

    pub(crate) fn plan(&self, observation: RecoveryObservation) -> RecoveryPlan {
        RecoveryPlanningPolicy::plan(&self.fact.durable, observation)
    }

    pub(crate) fn plan_resident(
        &self,
        command: ReplayCommand,
        observation: ResidentRecoveryObservation<'_>,
    ) -> ResidentRecoveryPlan {
        ResidentRecoveryPlanningPolicy::plan(&self.fact.durable, command, observation)
    }
}

#[derive(Clone)]
pub(crate) struct RecoveryOperationFact {
    pid: String,
    tid: String,
    durable: crate::kernel::DurableRecoveryFact,
    action: Option<RecoveredAction>,
}

impl RecoveryOperationFact {
    fn from_row(row: &data::Op) -> Result<Self, RecoveryFactError> {
        if row.pid.is_empty() {
            return Err(RecoveryFactError::MissingProcessId);
        }
        if row.tid.is_empty() {
            return Err(RecoveryFactError::MissingTaskId);
        }
        if !row.source_tid.is_empty() && row.source_tid != row.tid {
            return Err(RecoveryFactError::SourceTaskMismatch);
        }
        let operation_type =
            data::OpType::parse(&row.r#type).ok_or(RecoveryFactError::OperationType)?;
        let status =
            data::OpStatus::parse(&row.status).ok_or(RecoveryFactError::OperationStatus)?;
        let phase = match if row.phase.is_empty() {
            Some(data::OpPhase::DurablePending)
        } else {
            data::OpPhase::parse(&row.phase)
        } {
            Some(data::OpPhase::Created) => crate::kernel::OperationPhase::Created,
            Some(data::OpPhase::DurablePending) => crate::kernel::OperationPhase::DurablePending,
            Some(data::OpPhase::Dispatched) => crate::kernel::OperationPhase::Dispatched,
            Some(data::OpPhase::EffectInFlight) => crate::kernel::OperationPhase::EffectInFlight,
            Some(data::OpPhase::EffectDurable) => crate::kernel::OperationPhase::EffectDurable,
            Some(data::OpPhase::Completed) => crate::kernel::OperationPhase::Completed,
            None => return Err(RecoveryFactError::OperationPhase),
        };
        let action = match (operation_type, row.event.as_deref(), row.options.as_deref()) {
            (data::OpType::Action, Some(event), Some(options)) => Some(RecoveredAction {
                event: EventAction::parse(event).map_err(|_| RecoveryFactError::ActionEvent)?,
                options: serde_json::from_str(options)
                    .map_err(|_| RecoveryFactError::ActionOptions)?,
            }),
            (data::OpType::Action, _, _) => {
                return Err(RecoveryFactError::MissingActionPayload);
            }
            (_, None, None) => None,
            (_, _, _) => return Err(RecoveryFactError::UnexpectedActionPayload),
        };
        let replay_kind = match operation_type {
            data::OpType::Exec => ReplayOperationKind::Execute,
            data::OpType::Next => ReplayOperationKind::Propagate,
            data::OpType::Action => ReplayOperationKind::Action,
            data::OpType::Error => ReplayOperationKind::Error,
            data::OpType::Abort => ReplayOperationKind::Abort,
        };
        let replay_placement = match status {
            data::OpStatus::Pending => crate::kernel::ReplayPlacement::Pending,
            data::OpStatus::Dispatched => crate::kernel::ReplayPlacement::Dispatched,
            data::OpStatus::Overflow => crate::kernel::ReplayPlacement::Overflow,
            data::OpStatus::Done => crate::kernel::ReplayPlacement::Done,
        };
        Ok(Self {
            pid: row.pid.clone(),
            tid: row.tid.clone(),
            durable: crate::kernel::DurableRecoveryFact::new(
                replay_kind,
                replay_placement,
                phase,
                row.source_version,
                row.target_tid.clone().filter(|target| !target.is_empty()),
            ),
            action,
        })
    }

    pub(crate) fn action(&self) -> Option<&RecoveredAction> {
        self.action.as_ref()
    }

    pub(crate) fn replay_kind(&self) -> ReplayOperationKind {
        self.durable.kind()
    }

    pub(crate) fn replay_command(&self) -> Option<ReplayCommand> {
        self.durable.replay_command()
    }

    pub(crate) fn effect_is_durable(&self) -> bool {
        self.durable.effect_is_durable()
    }

    pub(crate) fn pid(&self) -> &str {
        &self.pid
    }

    pub(crate) fn tid(&self) -> &str {
        &self.tid
    }

    fn source_version(&self) -> i64 {
        self.durable.source_generation()
    }

    pub(crate) fn resolve_generation(&self, resident_generation: i64) -> i64 {
        self.durable.resolve_generation(resident_generation)
    }

    pub(crate) fn describes_handoff(
        &self,
        resident_generation: i64,
        resident_parent_tid: Option<&str>,
    ) -> bool {
        self.durable
            .describes_handoff(resident_generation, resident_parent_tid)
    }
}

#[cfg(test)]
mod recovery_fact_tests {
    use super::*;

    fn row(operation_type: data::OpType, status: data::OpStatus) -> data::Op {
        data::Op {
            id: "op".to_string(),
            pid: "pid".to_string(),
            source_tid: "tid".to_string(),
            tid: "tid".to_string(),
            target_tid: None,
            source_version: 0,
            causal_op_id: None,
            phase: data::OpPhase::DurablePending.as_ref().to_string(),
            r#type: operation_type.as_ref().to_string(),
            status: status.as_ref().to_string(),
            event: None,
            options: None,
            create_time: 0,
            update_time: 0,
            v: 0,
        }
    }

    fn fact(operation_type: data::OpType, status: data::OpStatus) -> RecoveryOperationFact {
        RecoveryOperationFact::from_row(&row(operation_type, status)).expect("valid fact")
    }

    #[test]
    fn operation_phase_requires_only_close_after_durable_effect() {
        let mut durable = row(data::OpType::Next, data::OpStatus::Pending);
        durable.phase = data::OpPhase::EffectDurable.as_ref().to_string();
        assert!(
            RecoveryOperationFact::from_row(&durable)
                .expect("durable fact")
                .effect_is_durable()
        );

        let mut in_flight = row(data::OpType::Next, data::OpStatus::Pending);
        in_flight.phase = data::OpPhase::EffectInFlight.as_ref().to_string();
        assert!(
            !RecoveryOperationFact::from_row(&in_flight)
                .expect("in-flight fact")
                .effect_is_durable()
        );
    }

    #[test]
    fn resident_operation_ownership_precedes_task_state() {
        assert_eq!(
            BootTaskPlanningPolicy::plan(BootTaskObservation {
                execution_state: crate::kernel::ExecutionState::Ready,
                has_children: false,
                durable_operation_owned: true
            }),
            crate::kernel::BootTaskPlan::AwaitDurableRecovery,
        );
        assert_eq!(
            BootTaskPlanningPolicy::plan(BootTaskObservation {
                execution_state: crate::kernel::ExecutionState::Created,
                has_children: false,
                durable_operation_owned: false
            }),
            crate::kernel::BootTaskPlan::Readmit,
        );
        assert_eq!(
            BootTaskPlanningPolicy::plan(BootTaskObservation {
                execution_state: crate::kernel::ExecutionState::Running,
                has_children: false,
                durable_operation_owned: false
            }),
            crate::kernel::BootTaskPlan::ResetThenReadmit,
        );
        assert_eq!(
            BootTaskPlanningPolicy::plan(BootTaskObservation {
                execution_state: crate::kernel::ExecutionState::Running,
                has_children: true,
                durable_operation_owned: false
            }),
            crate::kernel::BootTaskPlan::LeaveParked,
        );
        assert_eq!(
            BootTaskPlanningPolicy::plan(BootTaskObservation {
                execution_state: crate::kernel::ExecutionState::Completed,
                has_children: false,
                durable_operation_owned: false
            }),
            crate::kernel::BootTaskPlan::LeaveParked,
        );
    }

    #[test]
    fn scheduled_operation_classification_is_closed_and_status_aware() {
        assert_eq!(
            fact(data::OpType::Exec, data::OpStatus::Pending).replay_command(),
            Some(ReplayCommand::Execute),
        );
        assert_eq!(
            fact(data::OpType::Next, data::OpStatus::Dispatched).replay_command(),
            Some(ReplayCommand::Propagate),
        );
        // Open placements replay (the handoff is not durable, and the live
        // owner is excluded earlier by the confirmation observation); a Done
        // row is closed and never replays.
        assert_eq!(
            fact(data::OpType::Error, data::OpStatus::Dispatched).replay_command(),
            Some(ReplayCommand::Error),
        );
        assert_eq!(
            fact(data::OpType::Error, data::OpStatus::Done).replay_command(),
            None,
        );
        assert_eq!(
            fact(data::OpType::Abort, data::OpStatus::Overflow).replay_command(),
            Some(ReplayCommand::Abort),
        );
    }

    #[test]
    fn fact_adaptation_rejects_corrupt_or_inconsistent_rows() {
        let mut invalid = row(data::OpType::Exec, data::OpStatus::Pending);
        invalid.r#type = "unknown".to_string();
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::OperationType),
        );
        invalid = row(data::OpType::Exec, data::OpStatus::Pending);
        invalid.pid.clear();
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::MissingProcessId),
        );
        invalid = row(data::OpType::Exec, data::OpStatus::Pending);
        invalid.tid.clear();
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::MissingTaskId),
        );
        invalid = row(data::OpType::Next, data::OpStatus::Pending);
        invalid.source_tid = "different-task".to_string();
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::SourceTaskMismatch),
        );
        invalid = row(data::OpType::Exec, data::OpStatus::Pending);
        invalid.phase = "unknown".to_string();
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::OperationPhase),
        );
        invalid = row(data::OpType::Action, data::OpStatus::Pending);
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::MissingActionPayload),
        );
        invalid = row(data::OpType::Exec, data::OpStatus::Pending);
        invalid.event = Some("submit".to_string());
        assert_eq!(
            RecoveryOperationFact::from_row(&invalid).err(),
            Some(RecoveryFactError::UnexpectedActionPayload),
        );
    }

    #[test]
    fn fact_owns_typed_action_and_handoff_identity() {
        let mut action = row(data::OpType::Action, data::OpStatus::Pending);
        action.event = Some("submit".to_string());
        action.options = Some(r#"{"approved":true}"#.to_string());
        assert!(
            RecoveryOperationFact::from_row(&action)
                .expect("valid action fact")
                .action()
                .is_some()
        );

        let mut handoff = row(data::OpType::Next, data::OpStatus::Pending);
        handoff.source_version = 42;
        handoff.target_tid = Some("parent".to_string());
        let handoff = RecoveryOperationFact::from_row(&handoff).expect("valid handoff fact");
        assert_eq!(handoff.resolve_generation(7), 42);
        assert!(handoff.describes_handoff(42, Some("parent")));
        assert!(!handoff.describes_handoff(41, Some("parent")));
    }
    #[test]
    fn unknown_operation_phase_is_rejected() {
        let mut value = row(data::OpType::Next, data::OpStatus::Pending);
        value.phase = "corrupted-phase".to_string();
        assert_eq!(
            RecoveryCandidate::decode(&value).err(),
            Some(RecoveryFactError::OperationPhase)
        );
    }

    #[test]
    fn unknown_operation_type_is_rejected() {
        let mut value = row(data::OpType::Next, data::OpStatus::Pending);
        value.r#type = "future-operation".to_string();
        assert_eq!(
            RecoveryCandidate::decode(&value).err(),
            Some(RecoveryFactError::OperationType)
        );
    }
}

#[cfg(test)]
mod recovery_planning_tests {
    use super::*;
    use crate::kernel::OperationPhase;

    #[test]
    fn owned_confirmation_excludes_all_replay_placements() {
        for placement in [
            ReplayPlacement::Pending,
            ReplayPlacement::Overflow,
            ReplayPlacement::Dispatched,
        ] {
            assert_eq!(
                RecoveryPlanningPolicy::plan(
                    &DurableRecoveryFact::new(
                        ReplayOperationKind::Execute,
                        placement,
                        OperationPhase::DurablePending,
                        1,
                        None
                    ),
                    RecoveryObservation {
                        confirmation_owned: true
                    }
                ),
                RecoveryPlan::AwaitConfirmation
            );
        }
    }

    #[test]
    fn unowned_open_rows_are_replay_candidates_after_crash() {
        for placement in [
            ReplayPlacement::Pending,
            ReplayPlacement::Overflow,
            ReplayPlacement::Dispatched,
        ] {
            assert_eq!(
                RecoveryPlanningPolicy::plan(
                    &DurableRecoveryFact::new(
                        ReplayOperationKind::Execute,
                        placement,
                        OperationPhase::DurablePending,
                        1,
                        None
                    ),
                    RecoveryObservation {
                        confirmation_owned: false
                    }
                ),
                RecoveryPlan::InspectResident(ReplayCommand::Execute)
            );
        }
    }

    #[test]
    fn pending_actions_are_inspected_by_the_action_recovery_path() {
        assert_eq!(
            RecoveryPlanningPolicy::plan(
                &DurableRecoveryFact::new(
                    ReplayOperationKind::Action,
                    ReplayPlacement::Pending,
                    OperationPhase::DurablePending,
                    0,
                    None,
                ),
                RecoveryObservation {
                    confirmation_owned: false,
                },
            ),
            RecoveryPlan::InspectAction
        );
    }

    #[test]
    fn closed_rows_are_never_replayed() {
        assert_eq!(
            RecoveryPlanningPolicy::plan(
                &DurableRecoveryFact::new(
                    ReplayOperationKind::Execute,
                    ReplayPlacement::Done,
                    OperationPhase::Completed,
                    1,
                    None
                ),
                RecoveryObservation {
                    confirmation_owned: false
                }
            ),
            RecoveryPlan::IgnoreClosed
        );
    }
}
