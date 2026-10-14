use crate::scheduler::core::{
    model::{
        execution_graph::{ExecutionId, ExecutionState},
        model_graph::NodeId,
        operation::{CommitToken, OperationEvent, OperationRecord, OperationState},
        transition_plan::{AggregateVersion, TransitionPlan},
    },
    outbox::records::{OutboxId, OutboxRecord},
};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionSnapshot {
    pub(crate) model_node: NodeId,
    pub(crate) parent: Option<ExecutionId>,
    pub(crate) predecessors: Vec<ExecutionId>,
    pub(crate) state: ExecutionState,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AggregateSnapshot {
    pub(crate) process_id: String,
    pub(crate) version: AggregateVersion,
    pub(crate) executions: HashMap<ExecutionId, ExecutionSnapshot>,
    pub(crate) variables: BTreeMap<(ExecutionId, String), String>,
    pub(crate) operations:
        HashMap<crate::scheduler::core::model::operation::OperationId, OperationRecord>,
    pub(crate) outbox: HashMap<
        crate::scheduler::core::outbox::records::OutboxId,
        crate::scheduler::core::outbox::records::OutboxRecord,
    >,
}

impl AggregateSnapshot {
    /// The next free operation sequence for one execution: operation ids are
    /// unique per (task, generation), and an execution that already ran hops
    /// (a fan-out child, a rewound generation) owns earlier sequences, so
    /// every newly seeded operation takes the one after the highest known.
    pub(crate) fn next_operation_sequence(&self, task_id: &str, generation: u64) -> u64 {
        self.operations
            .keys()
            .filter(|id| id.task_id == task_id && id.generation == generation)
            .map(|id| id.sequence)
            .max()
            .map_or(0, |sequence| sequence + 1)
    }

    pub(crate) fn empty(process_id: impl Into<String>) -> Result<Self, CommitError> {
        let process_id = process_id.into();
        if process_id.trim().is_empty() {
            return Err(CommitError::EmptyProcessId);
        }
        Ok(Self {
            process_id,
            version: AggregateVersion {
                graph: 0,
                process: 0,
                operations: 0,
            },
            executions: HashMap::new(),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        })
    }

    pub(crate) fn validate(&self) -> Result<(), CommitError> {
        if self.process_id.trim().is_empty() {
            return Err(CommitError::EmptyProcessId);
        }
        for (id, operation) in &self.operations {
            if id != &operation.id {
                return Err(CommitError::OperationIdentityMismatch {
                    key: id.clone(),
                    record: operation.id.clone(),
                });
            }
            if id.process_id != self.process_id {
                return Err(CommitError::ForeignOperation(id.clone()));
            }
        }
        for (id, record) in &self.outbox {
            if id != &record.id {
                return Err(CommitError::OutboxIdentityMismatch {
                    key: id.clone(),
                    record: record.id.clone(),
                });
            }
            if id.process_id != self.process_id {
                return Err(CommitError::ForeignOutbox(id.clone()));
            }
            record.validate().map_err(CommitError::InvalidOutbox)?;
        }
        for (id, execution) in &self.executions {
            if let Some(parent) = &execution.parent
                && !self.executions.contains_key(parent)
            {
                return Err(CommitError::ExecutionNotFound(parent.clone()));
            }
            let mut predecessors = HashSet::new();
            for predecessor in &execution.predecessors {
                if predecessor == id || !predecessors.insert(predecessor.clone()) {
                    return Err(CommitError::InvalidExecutionTopology {
                        execution: id.clone(),
                        predecessor: predecessor.clone(),
                    });
                }
                if !self.executions.contains_key(predecessor) {
                    return Err(CommitError::ExecutionNotFound(predecessor.clone()));
                }
            }
        }
        for (execution, name) in self.variables.keys() {
            if !self.executions.contains_key(execution) {
                return Err(CommitError::ExecutionNotFound(execution.clone()));
            }
            if name.trim().is_empty() {
                return Err(CommitError::EmptyVariableName);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommitReceipt {
    pub(crate) token: CommitToken,
    pub(crate) previous: AggregateVersion,
    pub(crate) current: AggregateVersion,
    pub(crate) new_operations: Vec<crate::scheduler::core::model::operation::OperationId>,
    pub(crate) new_outbox: Vec<OutboxId>,
}

/// Storage boundary required by the kernel. Implementations must make compare
/// and commit one indivisible operation. A backend unable to do that must
/// refuse the commit rather than perform check-then-write.
#[cfg(test)]
pub(crate) trait AggregateRepository {
    fn load(&self, process_id: &str) -> Result<Option<AggregateSnapshot>, CommitError>;
    fn compare_and_commit(&mut self, plan: &TransitionPlan) -> Result<CommitReceipt, CommitError>;
}

/// Durable ownership boundary. Claiming and preparing are repository
/// transitions, not process-local bookkeeping. Recovery therefore observes
/// the same state as normal execution and never guesses from a local registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryCursor(pub(crate) crate::scheduler::core::model::operation::OperationId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryPage {
    pub(crate) operations: Vec<OperationRecord>,
    pub(crate) next: Option<RecoveryCursor>,
}

#[cfg(test)]
pub(crate) trait RecoveryRepository {
    fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, CommitError>;
}

#[cfg(test)]
pub(crate) trait OperationRepository {
    fn claim_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError>;

    fn renew_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        current: crate::scheduler::core::model::operation::ClaimToken,
        renewed: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError>;

    fn prepare_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError>;

    fn complete_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        commit: CommitToken,
    ) -> Result<OperationRecord, CommitError>;
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryAggregateRepository {
    aggregates: HashMap<String, AggregateSnapshot>,
}

#[cfg(test)]
impl MemoryAggregateRepository {
    pub(crate) fn insert(&mut self, snapshot: AggregateSnapshot) -> Result<(), CommitError> {
        snapshot.validate()?;
        if self.aggregates.contains_key(&snapshot.process_id) {
            return Err(CommitError::AggregateAlreadyExists(snapshot.process_id));
        }
        self.aggregates
            .insert(snapshot.process_id.clone(), snapshot);
        Ok(())
    }
}

#[cfg(test)]
impl RecoveryRepository for MemoryAggregateRepository {
    fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, CommitError> {
        if limit == 0 {
            return Err(CommitError::InvalidPageLimit);
        }
        let mut operations = self
            .aggregates
            .values()
            .flat_map(|snapshot| snapshot.operations.values().cloned())
            .filter(|operation| !operation.is_terminal())
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| operation_order(&left.id, &right.id));
        if let Some(cursor) = after {
            operations.retain(|operation| operation_order(&operation.id, &cursor.0).is_gt());
        }
        let has_more = operations.len() > limit;
        operations.truncate(limit);
        let next = if has_more {
            let last = operations.last().ok_or_else(|| {
                CommitError::CorruptAggregate(
                    "recovery pagination reported more rows for an empty page".into(),
                )
            })?;
            Some(RecoveryCursor(last.id.clone()))
        } else {
            None
        };
        Ok(RecoveryPage { operations, next })
    }
}

#[cfg(test)]
pub(crate) trait MemoryAggregateRepositoryAccess {
    fn insert_for_test(&mut self, snapshot: AggregateSnapshot) -> Result<(), CommitError>;
}

#[cfg(test)]
impl MemoryAggregateRepositoryAccess for MemoryAggregateRepository {
    fn insert_for_test(&mut self, snapshot: AggregateSnapshot) -> Result<(), CommitError> {
        self.insert(snapshot)
    }
}

#[cfg(test)]
impl OperationRepository for MemoryAggregateRepository {
    fn claim_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(id, expected_revision, OperationEvent::Claim(claim))
    }

    fn renew_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        current: crate::scheduler::core::model::operation::ClaimToken,
        renewed: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(
            id,
            expected_revision,
            OperationEvent::RenewClaim { current, renewed },
        )
    }

    fn prepare_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(
            id,
            expected_revision,
            OperationEvent::PrepareCommit { claim },
        )
    }

    fn complete_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        commit: CommitToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(id, expected_revision, OperationEvent::Complete { commit })
    }
}

/// Advance the operation lifecycle in place.
///
/// The durable repository owns the loaded snapshot, never reuses it after a
/// failed commit (it drops the aggregate and reports the failure, and recovery
/// re-derives the hop), so it can hand the snapshot over instead of paying for
/// a clone of the whole aggregate per commit. Every check runs before the
/// first mutation.
pub(crate) fn advance_operation_event(
    next: &mut AggregateSnapshot,
    id: &crate::scheduler::core::model::operation::OperationId,
    expected_revision: u64,
    event: OperationEvent,
) -> Result<OperationRecord, CommitError> {
    next.validate()?;
    if next.process_id != id.process_id {
        return Err(CommitError::ForeignOperation(id.clone()));
    }
    let operation = next
        .operations
        .get(id)
        .ok_or_else(|| CommitError::OperationNotFound(id.clone()))?;
    if operation.revision != expected_revision {
        return Err(CommitError::OperationRevisionConflict {
            operation: id.clone(),
            expected: expected_revision,
            actual: operation.revision,
        });
    }
    let next_operation = operation
        .evolve(event)
        .map_err(|_| CommitError::InvalidOperationState(operation.state.clone()))?;
    next.operations.insert(id.clone(), next_operation.clone());
    next.version.operations = next
        .version
        .operations
        .checked_add(1)
        .ok_or(CommitError::RevisionOverflow)?;
    next.validate()?;
    Ok(next_operation)
}

#[cfg(test)]
impl MemoryAggregateRepository {
    fn evolve_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<OperationRecord, CommitError> {
        let current = self
            .aggregates
            .get(&id.process_id)
            .ok_or_else(|| CommitError::AggregateNotFound(id.process_id.clone()))?;
        let mut next = current.clone();
        let operation = advance_operation_event(&mut next, id, expected_revision, event)?;
        self.aggregates.insert(id.process_id.clone(), next);
        Ok(operation)
    }
}

#[cfg(test)]
impl AggregateRepository for MemoryAggregateRepository {
    fn load(&self, process_id: &str) -> Result<Option<AggregateSnapshot>, CommitError> {
        let snapshot = self.aggregates.get(process_id).cloned();
        if let Some(value) = &snapshot {
            value.validate()?;
        }
        Ok(snapshot)
    }

    fn compare_and_commit(&mut self, plan: &TransitionPlan) -> Result<CommitReceipt, CommitError> {
        let current = self
            .aggregates
            .get(&plan.process_id)
            .ok_or_else(|| CommitError::AggregateNotFound(plan.process_id.clone()))?;
        let mut next = current.clone();
        let receipt = advance_transition(&mut next, plan)?;
        self.aggregates.insert(plan.process_id.clone(), next);
        Ok(receipt)
    }
}

/// Advance one aggregate in place (see [`advance_operation_event`] for why the
/// durable repository may hand its snapshot over).
pub(crate) fn advance_transition(
    next: &mut AggregateSnapshot,
    plan: &TransitionPlan,
) -> Result<CommitReceipt, CommitError> {
    plan.validate()
        .map_err(|error| CommitError::InvalidPlan(error.to_string()))?;
    next.validate()?;
    validate_preconditions(next, plan)?;

    let previous = next.version.clone();
    for create in &plan.execution_creates {
        next.executions.insert(
            create.execution.clone(),
            ExecutionSnapshot {
                model_node: create.model_node.clone(),
                parent: create.parent.clone(),
                predecessors: create.predecessors.clone(),
                state: create.initial_state,
                revision: 0,
            },
        );
    }
    for change in &plan.state_changes {
        let execution = next
            .executions
            .get_mut(&change.execution)
            .ok_or_else(|| CommitError::ExecutionNotFound(change.execution.clone()))?;
        execution.state = change.to;
        execution.revision = execution
            .revision
            .checked_add(1)
            .ok_or(CommitError::RevisionOverflow)?;
    }
    for write in &plan.variable_writes {
        next.variables.insert(
            (write.scope.clone(), write.name.clone()),
            write.value.clone(),
        );
    }

    let mut new_operations = Vec::with_capacity(plan.operations.len());
    for intent in &plan.operations {
        next.operations.insert(
            intent.id.clone(),
            OperationRecord::pending(intent.id.clone(), intent.kind),
        );
        new_operations.push(intent.id.clone());
    }
    let mut new_outbox = Vec::with_capacity(plan.outbox.len());
    for item in &plan.outbox {
        let id = OutboxId {
            process_id: next.process_id.clone(),
            key: item.key.clone(),
        };
        let record = OutboxRecord::pending(id.clone(), item.topic.clone(), item.payload.clone(), 0)
            .map_err(CommitError::InvalidOutbox)?;
        new_outbox.push(id.clone());
        next.outbox.insert(id, record);
    }

    let graph_delta = u64::from(!plan.execution_creates.is_empty());
    let process_delta =
        u64::from(!plan.state_changes.is_empty() || !plan.variable_writes.is_empty());
    let operation_delta = u64::try_from(plan.operations.len())
        .map_err(|_| CommitError::RevisionOverflow)?
        .checked_add(u64::from(plan.source_operation.is_some()))
        .ok_or(CommitError::RevisionOverflow)?;
    next.version.graph = next
        .version
        .graph
        .checked_add(graph_delta)
        .ok_or(CommitError::RevisionOverflow)?;
    next.version.process = next
        .version
        .process
        .checked_add(process_delta)
        .ok_or(CommitError::RevisionOverflow)?;
    next.version.operations = next
        .version
        .operations
        .checked_add(operation_delta)
        .ok_or(CommitError::RevisionOverflow)?;

    let current_version = next.version.clone();
    let token = CommitToken {
        process_version: current_version.process,
        digest: deterministic_digest(next)?,
    };
    if let Some(source) = &plan.source_operation {
        let operation = next
            .operations
            .get(&source.id)
            .ok_or_else(|| CommitError::OperationNotFound(source.id.clone()))?;
        let committed = operation
            .evolve(OperationEvent::Commit {
                claim: source.claim.clone(),
                commit: token.clone(),
            })
            .map_err(|_| CommitError::InvalidOperationState(operation.state.clone()))?;
        next.operations.insert(source.id.clone(), committed);
    }
    next.validate()?;
    Ok(CommitReceipt {
        token,
        previous,
        current: current_version,
        new_operations,
        new_outbox,
    })
}

fn validate_preconditions(
    snapshot: &AggregateSnapshot,
    plan: &TransitionPlan,
) -> Result<(), CommitError> {
    if snapshot.version != plan.expected {
        return Err(CommitError::VersionConflict {
            expected: plan.expected.clone(),
            actual: snapshot.version.clone(),
        });
    }
    let created = plan
        .execution_creates
        .iter()
        .map(|item| item.execution.clone())
        .collect::<HashSet<_>>();
    for create in &plan.execution_creates {
        if snapshot.executions.contains_key(&create.execution) {
            return Err(CommitError::ExecutionAlreadyExists(
                create.execution.clone(),
            ));
        }
        if let Some(parent) = &create.parent
            && !snapshot.executions.contains_key(parent)
            && !created.contains(parent)
        {
            return Err(CommitError::ExecutionNotFound(parent.clone()));
        }
        for predecessor in &create.predecessors {
            if !snapshot.executions.contains_key(predecessor) && !created.contains(predecessor) {
                return Err(CommitError::ExecutionNotFound(predecessor.clone()));
            }
        }
        if snapshot.executions.keys().any(|id| {
            id.task_id == create.execution.task_id && id.generation >= create.execution.generation
        }) {
            return Err(CommitError::GenerationNotMonotonic(
                create.execution.clone(),
            ));
        }
    }
    for change in &plan.state_changes {
        let current = snapshot
            .executions
            .get(&change.execution)
            .ok_or_else(|| CommitError::ExecutionNotFound(change.execution.clone()))?;
        if current.revision != change.expected_revision || current.state != change.from {
            return Err(CommitError::ExecutionConflict {
                execution: change.execution.clone(),
                expected_revision: change.expected_revision,
                actual_revision: current.revision,
                expected_state: change.from,
                actual_state: current.state,
            });
        }
    }
    for write in &plan.variable_writes {
        // A recreated generation is created and seeded with variables in the
        // same plan: the scope exists once the creates install.
        if !snapshot.executions.contains_key(&write.scope) && !created.contains(&write.scope) {
            return Err(CommitError::ExecutionNotFound(write.scope.clone()));
        }
    }
    if let Some(source) = &plan.source_operation {
        let operation = snapshot
            .operations
            .get(&source.id)
            .ok_or_else(|| CommitError::OperationNotFound(source.id.clone()))?;
        if operation.state != OperationState::CommitPrepared(source.claim.clone()) {
            return Err(CommitError::InvalidOperationState(operation.state.clone()));
        }
    }
    for intent in &plan.operations {
        if snapshot.operations.contains_key(&intent.id) {
            return Err(CommitError::OperationAlreadyExists(intent.id.clone()));
        }
    }
    for item in &plan.outbox {
        let id = OutboxId {
            process_id: snapshot.process_id.clone(),
            key: item.key.clone(),
        };
        if snapshot.outbox.contains_key(&id) {
            return Err(CommitError::OutboxAlreadyExists(item.key.clone()));
        }
    }
    Ok(())
}

fn operation_order(
    left: &crate::scheduler::core::model::operation::OperationId,
    right: &crate::scheduler::core::model::operation::OperationId,
) -> std::cmp::Ordering {
    left.process_id
        .cmp(&right.process_id)
        .then_with(|| left.task_id.cmp(&right.task_id))
        .then_with(|| left.generation.cmp(&right.generation))
        .then_with(|| left.sequence.cmp(&right.sequence))
}

fn execution_state_tag(state: ExecutionState) -> &'static str {
    match state {
        ExecutionState::Created => "Created",
        ExecutionState::Ready => "Ready",
        ExecutionState::Running => "Running",
        ExecutionState::Waiting => "Waiting",
        ExecutionState::Completed => "Completed",
        ExecutionState::Submitted => "Submitted",
        ExecutionState::Backed => "Backed",
        ExecutionState::Cancelled => "Cancelled",
        ExecutionState::Failed => "Failed",
        ExecutionState::Aborted => "Aborted",
        ExecutionState::Skipped => "Skipped",
        ExecutionState::Removed => "Removed",
    }
}

fn operation_kind_tag(
    kind: crate::scheduler::core::model::operation::OperationKind,
) -> &'static str {
    match kind {
        crate::scheduler::core::model::operation::OperationKind::ExecuteTask => "ExecuteTask",
        crate::scheduler::core::model::operation::OperationKind::PropagateTask => "PropagateTask",
        crate::scheduler::core::model::operation::OperationKind::ApplyAction => "ApplyAction",
        crate::scheduler::core::model::operation::OperationKind::SkipTask => "SkipTask",
        crate::scheduler::core::model::operation::OperationKind::AbortTask => "AbortTask",
        crate::scheduler::core::model::operation::OperationKind::DispatchTimeout => {
            "DispatchTimeout"
        }
        crate::scheduler::core::model::operation::OperationKind::ReenterTask => "ReenterTask",
        crate::scheduler::core::model::operation::OperationKind::DeliverMessage => "DeliverMessage",
        crate::scheduler::core::model::operation::OperationKind::FireTrigger => "FireTrigger",
        crate::scheduler::core::model::operation::OperationKind::RunPostCommit => "RunPostCommit",
    }
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.escape_debug())
}

fn claim_token_tag(claim: &crate::scheduler::core::model::operation::ClaimToken) -> String {
    format!(
        "ClaimToken {{ owner: {}, fence: {}, lease_until_ms: {}, heartbeat_revision: {} }}",
        quoted(&claim.owner),
        claim.fence,
        claim.lease_until_ms,
        claim.heartbeat_revision
    )
}

fn commit_token_tag(commit: &crate::scheduler::core::model::operation::CommitToken) -> String {
    format!(
        "CommitToken {{ process_version: {}, digest: {} }}",
        commit.process_version,
        quoted(&commit.digest)
    )
}

fn operation_state_tag(state: &crate::scheduler::core::model::operation::OperationState) -> String {
    match state {
        crate::scheduler::core::model::operation::OperationState::Pending => "Pending".into(),
        crate::scheduler::core::model::operation::OperationState::Claimed(claim) => {
            format!("Claimed({})", claim_token_tag(claim))
        }
        crate::scheduler::core::model::operation::OperationState::CommitPrepared(claim) => {
            format!("CommitPrepared({})", claim_token_tag(claim))
        }
        crate::scheduler::core::model::operation::OperationState::Committed(commit) => {
            format!("Committed({})", commit_token_tag(commit))
        }
        crate::scheduler::core::model::operation::OperationState::RetryScheduled { attempt } => {
            format!("RetryScheduled {{ attempt: {attempt} }}")
        }
        crate::scheduler::core::model::operation::OperationState::EffectUnknown { attempt } => {
            format!("EffectUnknown {{ attempt: {attempt} }}")
        }
        crate::scheduler::core::model::operation::OperationState::Quarantined { reason } => {
            format!("Quarantined {{ reason: {} }}", quoted(reason))
        }
        crate::scheduler::core::model::operation::OperationState::Superseded { by_generation } => {
            format!("Superseded {{ by_generation: {by_generation} }}")
        }
        crate::scheduler::core::model::operation::OperationState::Completed(commit) => {
            format!("Completed({})", commit_token_tag(commit))
        }
    }
}

pub(crate) fn canonical_aggregate_bytes(
    snapshot: &AggregateSnapshot,
) -> Result<Vec<u8>, CommitError> {
    // Canonical, unambiguous change token over every persisted field. Strings
    // are length-prefixed so user data containing separators cannot collide.
    fn field(output: &mut String, value: &str) {
        output.push_str(&value.len().to_string());
        output.push(':');
        output.push_str(value);
    }
    fn execution_id(output: &mut String, id: &ExecutionId) {
        field(output, &id.task_id);
        field(output, &id.generation.to_string());
    }

    let mut output = String::new();
    field(&mut output, "aggregate-v1");
    field(&mut output, &snapshot.process_id);
    field(&mut output, &snapshot.version.graph.to_string());
    field(&mut output, &snapshot.version.process.to_string());
    field(&mut output, &snapshot.version.operations.to_string());

    let mut executions = snapshot.executions.iter().collect::<Vec<_>>();
    executions.sort_by_key(|(id, _)| *id);
    for (id, value) in executions {
        field(&mut output, "execution");
        execution_id(&mut output, id);
        field(&mut output, value.model_node.as_str());
        match &value.parent {
            Some(parent) => {
                field(&mut output, "parent");
                execution_id(&mut output, parent);
            }
            None => field(&mut output, "no-parent"),
        }
        let mut predecessors = value.predecessors.clone();
        predecessors.sort();
        field(&mut output, &predecessors.len().to_string());
        for predecessor in predecessors {
            execution_id(&mut output, &predecessor);
        }
        field(&mut output, execution_state_tag(value.state));
        field(&mut output, &value.revision.to_string());
    }
    for ((id, name), value) in &snapshot.variables {
        field(&mut output, "variable");
        execution_id(&mut output, id);
        field(&mut output, name);
        field(&mut output, value);
    }
    let mut operations = snapshot.operations.values().collect::<Vec<_>>();
    operations.sort_by(|left, right| operation_order(&left.id, &right.id));
    for operation in operations {
        field(&mut output, "operation");
        field(&mut output, &operation.id.process_id);
        field(&mut output, &operation.id.task_id);
        field(&mut output, &operation.id.generation.to_string());
        field(&mut output, &operation.id.sequence.to_string());
        field(&mut output, operation_kind_tag(operation.kind));
        field(&mut output, &operation_state_tag(&operation.state));
        field(&mut output, &operation.revision.to_string());
    }
    let mut outbox = snapshot.outbox.values().collect::<Vec<_>>();
    outbox.sort_by(|left, right| left.id.cmp(&right.id));
    for record in outbox {
        let encoded = crate::scheduler::core::outbox::outbox_codec::OutboxCodec::encode(record)
            .map_err(|error| CommitError::OutboxEncoding(error.to_string()))?;
        field(&mut output, "outbox");
        field(&mut output, &sha256_hex(&encoded));
    }
    Ok(output.into_bytes())
}

fn deterministic_digest(snapshot: &AggregateSnapshot) -> Result<String, CommitError> {
    Ok(sha256_hex(&canonical_aggregate_bytes(snapshot)?))
}

pub(crate) fn sha256_hex(input: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommitError {
    EmptyProcessId,
    #[cfg(test)]
    AggregateAlreadyExists(String),
    AggregateNotFound(String),
    InvalidPlan(String),
    VersionConflict {
        expected: AggregateVersion,
        actual: AggregateVersion,
    },
    ExecutionNotFound(ExecutionId),
    ExecutionAlreadyExists(ExecutionId),
    GenerationNotMonotonic(ExecutionId),
    ExecutionConflict {
        execution: ExecutionId,
        expected_revision: u64,
        actual_revision: u64,
        expected_state: ExecutionState,
        actual_state: ExecutionState,
    },
    OperationNotFound(crate::scheduler::core::model::operation::OperationId),
    OperationIdentityMismatch {
        key: crate::scheduler::core::model::operation::OperationId,
        record: crate::scheduler::core::model::operation::OperationId,
    },
    ForeignOperation(crate::scheduler::core::model::operation::OperationId),
    InvalidExecutionTopology {
        execution: ExecutionId,
        predecessor: ExecutionId,
    },
    EmptyVariableName,
    OperationRevisionConflict {
        operation: crate::scheduler::core::model::operation::OperationId,
        expected: u64,
        actual: u64,
    },
    OperationAlreadyExists(crate::scheduler::core::model::operation::OperationId),
    OutboxAlreadyExists(String),
    OutboxIdentityMismatch {
        key: OutboxId,
        record: OutboxId,
    },
    ForeignOutbox(OutboxId),
    InvalidOutbox(crate::scheduler::core::outbox::records::OutboxError),
    OutboxEncoding(String),
    RevisionOverflow,
    InvalidPageLimit,
    InvalidOperationState(OperationState),
    CorruptAggregate(String),
}
impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "commit error: {self:?}")
    }
}
impl std::error::Error for CommitError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::operation::OperationId;
    use crate::scheduler::core::model::operation::OperationKind;
    use crate::scheduler::core::model::transition_plan::OperationIntent;
    use crate::scheduler::core::model::transition_plan::OutboxIntent;
    use crate::scheduler::core::model::transition_plan::StateChange;
    use crate::scheduler::core::model::transition_plan::VariableWrite;

    fn id() -> ExecutionId {
        ExecutionId::new("t1", 0).unwrap()
    }
    fn repository() -> MemoryAggregateRepository {
        let mut snapshot = AggregateSnapshot::empty("p1").unwrap();
        snapshot.executions.insert(
            id(),
            ExecutionSnapshot {
                model_node: NodeId::parse("t1").unwrap(),
                parent: None,
                predecessors: vec![],
                state: ExecutionState::Running,
                revision: 2,
            },
        );
        let mut repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();
        repo
    }
    fn plan() -> TransitionPlan {
        TransitionPlan {
            process_id: "p1".into(),
            expected: AggregateVersion {
                graph: 0,
                process: 0,
                operations: 0,
            },
            execution_creates: Vec::new(),
            state_changes: vec![StateChange {
                execution: id(),
                expected_revision: 2,
                from: ExecutionState::Running,
                to: ExecutionState::Completed,
            }],
            variable_writes: vec![VariableWrite {
                scope: id(),
                name: "result".into(),
                value: "42".into(),
            }],
            source_operation: None,
            operations: vec![OperationIntent {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "t1".into(),
                    generation: 0,
                    sequence: 1,
                },
                kind: OperationKind::PropagateTask,
            }],
            outbox: vec![OutboxIntent {
                key: "complete:p1:t1:0".into(),
                topic: "task.completed".into(),
                payload: "{}".into(),
            }],
        }
    }

    #[test]
    fn commit_is_atomic_across_state_vars_operations_and_outbox() {
        let mut repo = repository();
        let receipt = repo.compare_and_commit(&plan()).unwrap();
        let snapshot = repo.load("p1").unwrap().unwrap();
        assert_eq!(snapshot.executions[&id()].state, ExecutionState::Completed);
        assert_eq!(snapshot.variables[&(id(), "result".into())], "42");
        assert_eq!(snapshot.operations.len(), 1);
        assert!(
            snapshot
                .outbox
                .keys()
                .any(|id| &id.key == "complete:p1:t1:0")
        );
        assert_eq!(
            receipt.current,
            AggregateVersion {
                graph: 0,
                process: 1,
                operations: 1
            }
        );
    }

    #[test]
    fn conflict_changes_nothing() {
        let mut repo = repository();
        let mut invalid = plan();
        invalid.state_changes[0].expected_revision = 1;
        assert!(matches!(
            repo.compare_and_commit(&invalid),
            Err(CommitError::ExecutionConflict { .. })
        ));
        assert_eq!(
            repo.load("p1").unwrap().unwrap(),
            repository().load("p1").unwrap().unwrap()
        );
    }

    #[test]
    fn retrying_same_plan_is_rejected_without_duplicate_effects() {
        let mut repo = repository();
        repo.compare_and_commit(&plan()).unwrap();
        assert!(matches!(
            repo.compare_and_commit(&plan()),
            Err(CommitError::VersionConflict { .. })
        ));
        let snapshot = repo.load("p1").unwrap().unwrap();
        assert_eq!(snapshot.operations.len(), 1);
        assert_eq!(snapshot.outbox.len(), 1);
    }
    #[test]
    fn source_operation_is_committed_in_same_atomic_batch() {
        use crate::scheduler::core::model::operation::ClaimToken;
        use crate::scheduler::core::model::operation::OperationEvent;
        use crate::scheduler::core::model::transition_plan::OperationCommit;
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 9,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let source_id = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 0,
        };
        let mut repo = repository();
        let mut snapshot = repo.load("p1").unwrap().unwrap();
        let prepared = OperationRecord::pending(source_id.clone(), OperationKind::ApplyAction)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit {
                claim: claim.clone(),
            })
            .unwrap();
        snapshot.operations.insert(source_id.clone(), prepared);
        repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();

        let mut p = plan();
        p.source_operation = Some(OperationCommit {
            id: source_id.clone(),
            claim,
        });
        let receipt = repo.compare_and_commit(&p).unwrap();
        let snapshot = repo.load("p1").unwrap().unwrap();
        assert_eq!(
            snapshot.operations[&source_id].state,
            OperationState::Committed(receipt.token)
        );
        assert_eq!(snapshot.executions[&id()].state, ExecutionState::Completed);
    }

    #[test]
    fn stale_source_claim_rejects_whole_batch() {
        use crate::scheduler::core::model::operation::ClaimToken;
        use crate::scheduler::core::model::operation::OperationEvent;
        use crate::scheduler::core::model::transition_plan::OperationCommit;
        let active = ClaimToken {
            owner: "runtime-a".into(),
            fence: 9,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let stale = ClaimToken {
            owner: "runtime-b".into(),
            fence: 8,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let source_id = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 0,
        };
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        let prepared = OperationRecord::pending(source_id.clone(), OperationKind::ApplyAction)
            .evolve(OperationEvent::Claim(active.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit { claim: active })
            .unwrap();
        snapshot.operations.insert(source_id.clone(), prepared);
        let before = snapshot.clone();
        let mut repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();
        let mut p = plan();
        p.source_operation = Some(OperationCommit {
            id: source_id,
            claim: stale,
        });
        assert!(matches!(
            repo.compare_and_commit(&p),
            Err(CommitError::InvalidOperationState(_))
        ));
        assert_eq!(repo.load("p1").unwrap().unwrap(), before);
    }

    #[test]
    fn durable_claim_prepare_commit_complete_has_one_revision_chain() {
        use crate::scheduler::core::model::operation::ClaimToken;
        let id = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 0,
        };
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 3,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ApplyAction),
        );
        let mut repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();

        let claimed = repo.claim_operation(&id, 0, claim.clone()).unwrap();
        let prepared = repo
            .prepare_operation(&id, claimed.revision, claim.clone())
            .unwrap();
        assert_eq!(
            prepared.state,
            OperationState::CommitPrepared(claim.clone())
        );

        let mut p = plan();
        p.expected = repo.load("p1").unwrap().unwrap().version;
        p.source_operation = Some(
            crate::scheduler::core::model::transition_plan::OperationCommit {
                id: id.clone(),
                claim,
            },
        );
        let receipt = repo.compare_and_commit(&p).unwrap();
        let committed = repo.load("p1").unwrap().unwrap().operations[&id].clone();
        let completed = repo
            .complete_operation(&id, committed.revision, receipt.token.clone())
            .unwrap();
        assert_eq!(completed.state, OperationState::Completed(receipt.token));
    }

    #[test]
    fn operation_revision_conflict_does_not_advance_state() {
        use crate::scheduler::core::model::operation::ClaimToken;
        let id = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 0,
        };
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ApplyAction),
        );
        let mut repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();
        let error = repo
            .claim_operation(
                &id,
                9,
                ClaimToken {
                    owner: "r".into(),
                    fence: 1,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            CommitError::OperationRevisionConflict { .. }
        ));
        assert_eq!(
            repo.load("p1").unwrap().unwrap().operations[&id].state,
            OperationState::Pending
        );
    }

    #[test]
    fn execution_creation_advances_graph_and_process_atomically() {
        use crate::scheduler::core::model::transition_plan::ExecutionCreate;
        let mut repo = repository();
        let mut p = plan();
        p.execution_creates.push(ExecutionCreate {
            execution: ExecutionId::new("t1", 1).unwrap(),
            model_node: NodeId::parse("t1").unwrap(),
            parent: None,
            predecessors: vec![id()],
            initial_state: ExecutionState::Ready,
        });
        let receipt = repo.compare_and_commit(&p).unwrap();
        assert_eq!(receipt.current.graph, 1);
        assert_eq!(receipt.current.process, 1);
        let snapshot = repo.load("p1").unwrap().unwrap();
        assert_eq!(
            snapshot.executions[&ExecutionId::new("t1", 1).unwrap()].state,
            ExecutionState::Ready
        );
    }

    #[test]
    fn generation_must_move_forward_without_overwriting_history() {
        use crate::scheduler::core::model::transition_plan::ExecutionCreate;
        let mut repo = repository();
        let mut p = plan();
        p.execution_creates.push(ExecutionCreate {
            execution: id(),
            model_node: NodeId::parse("t1").unwrap(),
            parent: None,
            predecessors: vec![],
            initial_state: ExecutionState::Ready,
        });
        assert!(matches!(
            repo.compare_and_commit(&p),
            Err(CommitError::ExecutionAlreadyExists(_))
        ));
        assert_eq!(repo.load("p1").unwrap().unwrap().executions.len(), 1);
    }

    #[test]
    fn repository_heartbeat_uses_operation_revision_cas() {
        let id = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 0,
        };
        let current = ClaimToken {
            owner: "runtime-a".into(),
            fence: 4,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let renewed = ClaimToken {
            owner: current.owner.clone(),
            fence: current.fence,
            lease_until_ms: 200,
            heartbeat_revision: 1,
        };
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ExecuteTask),
        );
        let mut repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();
        let claimed = repo.claim_operation(&id, 0, current.clone()).unwrap();
        let heartbeat = repo
            .renew_operation(&id, claimed.revision, current.clone(), renewed.clone())
            .unwrap();
        assert_eq!(heartbeat.state, OperationState::Claimed(renewed));
        assert!(matches!(
            repo.renew_operation(&id, claimed.revision, current.clone(), current),
            Err(CommitError::OperationRevisionConflict { .. })
        ));
    }

    #[test]
    fn aggregate_rejects_operation_key_value_identity_mismatch() {
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        let key = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 10,
        };
        let record_id = OperationId {
            sequence: 11,
            ..key.clone()
        };
        snapshot.operations.insert(
            key.clone(),
            OperationRecord::pending(record_id.clone(), OperationKind::ExecuteTask),
        );
        assert_eq!(
            snapshot.validate(),
            Err(CommitError::OperationIdentityMismatch {
                key,
                record: record_id
            })
        );
    }

    #[test]
    fn aggregate_rejects_foreign_operation_and_dangling_variable_scope() {
        let mut foreign = repository().load("p1").unwrap().unwrap();
        let foreign_id = OperationId {
            process_id: "other".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 1,
        };
        foreign.operations.insert(
            foreign_id.clone(),
            OperationRecord::pending(foreign_id.clone(), OperationKind::ExecuteTask),
        );
        assert_eq!(
            foreign.validate(),
            Err(CommitError::ForeignOperation(foreign_id))
        );

        let mut dangling = repository().load("p1").unwrap().unwrap();
        let missing = ExecutionId::new("missing", 0).unwrap();
        dangling
            .variables
            .insert((missing.clone(), "value".into()), "1".into());
        assert_eq!(
            dangling.validate(),
            Err(CommitError::ExecutionNotFound(missing))
        );
    }

    #[test]
    fn recovery_pages_are_stable_non_overlapping_and_skip_terminal_rows() {
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        let claim = ClaimToken {
            owner: "r".into(),
            fence: 1,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        for sequence in 0..5 {
            let id = OperationId {
                process_id: "p1".into(),
                task_id: "t1".into(),
                generation: 0,
                sequence,
            };
            let mut record = OperationRecord::pending(id.clone(), OperationKind::ExecuteTask);
            if sequence == 2 {
                let commit = CommitToken {
                    process_version: 1,
                    digest: "d".into(),
                };
                record = record
                    .evolve(OperationEvent::Claim(claim.clone()))
                    .unwrap()
                    .evolve(OperationEvent::PrepareCommit {
                        claim: claim.clone(),
                    })
                    .unwrap()
                    .evolve(OperationEvent::Commit {
                        claim: claim.clone(),
                        commit: commit.clone(),
                    })
                    .unwrap()
                    .evolve(OperationEvent::Complete { commit })
                    .unwrap();
            }
            snapshot.operations.insert(id, record);
        }
        let mut repo = MemoryAggregateRepository::default();
        repo.insert(snapshot).unwrap();
        let first = repo.recoverable_page(None, 2).unwrap();
        let second = repo.recoverable_page(first.next.as_ref(), 2).unwrap();
        assert_eq!(
            first
                .operations
                .iter()
                .map(|op| op.id.sequence)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            second
                .operations
                .iter()
                .map(|op| op.id.sequence)
                .collect::<Vec<_>>(),
            [3, 4]
        );
        assert!(second.next.is_none());
        assert_eq!(
            repo.recoverable_page(None, 0),
            Err(CommitError::InvalidPageLimit)
        );
    }

    #[test]
    fn commit_digest_covers_graph_topology_and_model_identity() {
        let mut left = repository().load("p1").unwrap().unwrap();
        let mut right = left.clone();
        let execution = id();
        right.executions.get_mut(&execution).unwrap().model_node =
            NodeId::parse("different-model").unwrap();
        assert_ne!(
            deterministic_digest(&left).unwrap(),
            deterministic_digest(&right).unwrap()
        );
        left.version.graph = 9;
        assert_ne!(
            deterministic_digest(&left).unwrap(),
            deterministic_digest(&right).unwrap()
        );
    }

    #[test]
    fn commit_digest_is_unambiguous_for_separator_like_user_data() {
        let mut left = repository().load("p1").unwrap().unwrap();
        let mut right = left.clone();
        left.variables.insert((id(), "a:b".into()), "c".into());
        right.variables.insert((id(), "a".into()), "b:c".into());
        assert_ne!(
            deterministic_digest(&left).unwrap(),
            deterministic_digest(&right).unwrap()
        );
    }

    #[test]
    fn sha256_matches_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn commit_digest_is_fixed_size_lower_hex() {
        let digest = deterministic_digest(&repository().load("p1").unwrap().unwrap()).unwrap();
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn transition_advances_the_snapshot_it_is_given() {
        let repo = repository();
        let current = repo.load("p1").unwrap().unwrap();
        let before = current.clone();
        let mut next = current;
        let receipt = advance_transition(&mut next, &plan()).unwrap();
        assert_ne!(next, before);
        assert_eq!(receipt.previous, before.version);
        assert_eq!(receipt.current, next.version);
    }

    fn pending_operation_snapshot() -> (AggregateSnapshot, OperationId, ClaimToken) {
        let claim = ClaimToken {
            owner: "runtime".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let mut current = repository().load("p1").unwrap().unwrap();
        let operation_id = OperationId {
            process_id: "p1".into(),
            task_id: "t1".into(),
            generation: 0,
            sequence: 9,
        };
        current.operations.insert(
            operation_id.clone(),
            OperationRecord::pending(operation_id.clone(), OperationKind::ExecuteTask),
        );
        (current, operation_id, claim)
    }

    #[test]
    fn operation_event_advances_the_snapshot_it_is_given() {
        let (current, operation_id, claim) = pending_operation_snapshot();
        let before = current.clone();
        let mut next = current;
        let operation = advance_operation_event(
            &mut next,
            &operation_id,
            0,
            OperationEvent::Claim(claim.clone()),
        )
        .unwrap();
        assert_eq!(operation.state, OperationState::Claimed(claim));
        assert_eq!(operation.revision, 1);
        assert_eq!(next.version.operations, before.version.operations + 1);
        assert_eq!(next.operations[&operation_id], operation);
    }

    #[test]
    fn rejected_operation_event_leaves_its_snapshot_untouched() {
        // The reachable rejection runs before the first mutation, which is
        // what lets the durable repository hand its own snapshot over: a hop
        // that loses the revision race must leave the aggregate as it was.
        let (current, operation_id, claim) = pending_operation_snapshot();
        let mut next = current.clone();
        let outcome =
            advance_operation_event(&mut next, &operation_id, 7, OperationEvent::Claim(claim));
        assert!(outcome.is_err());
        assert_eq!(next, current);
    }
    #[test]
    fn canonical_aggregate_and_commit_digest_match_golden_fingerprint() {
        let snapshot = repository().load("p1").unwrap().unwrap();
        let bytes = canonical_aggregate_bytes(&snapshot).unwrap();
        const GOLDEN_DIGEST: &str =
            "a47d4bd489ffc8d817e121a5d1743a93b612d8bdd7da8a1ee2c5e1288967028e";
        assert_eq!(bytes.len(), 76);
        assert_eq!(sha256_hex(&bytes), GOLDEN_DIGEST);
        assert_eq!(deterministic_digest(&snapshot).unwrap(), GOLDEN_DIGEST);
    }
}
