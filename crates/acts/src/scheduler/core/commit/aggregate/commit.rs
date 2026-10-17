use super::*;

pub(crate) fn advance_operation_event(
    next: &mut ProcessSnapshot,
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

/// Advance one aggregate in place (see [`advance_operation_event`] for why the
/// durable repository may hand its snapshot over).
pub(crate) fn advance_transition(
    next: &mut ProcessSnapshot,
    plan: &TransitionPlan,
) -> Result<CommitReceipt, CommitError> {
    plan.validate()
        .map_err(|error| CommitError::InvalidPlan(error.to_string()))?;
    next.validate()?;
    validate_preconditions(next, plan)?;

    let previous = next.version.clone();
    for create in &plan.task_creates {
        next.tasks.insert(
            create.task.clone(),
            TaskSnapshot {
                model_node: create.model_node.clone(),
                parent: create.parent.clone(),
                predecessors: create.predecessors.clone(),
                state: create.initial_state,
                revision: 0,
            },
        );
    }
    for change in &plan.state_changes {
        let task = next
            .tasks
            .get_mut(&change.task)
            .ok_or_else(|| CommitError::TaskNotFound(change.task.clone()))?;
        task.state = change.to;
        task.revision = task
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
        let mut record =
            OutboxRecord::pending(id.clone(), item.topic.clone(), item.payload.clone(), 0)
                .map_err(CommitError::InvalidOutbox)?;
        // A record the operation's own transport already delivered commits as
        // delivered (see `OutboxIntent::receipt`): the message is out, and the
        // record exists to say so — the driver's pass never sees it, so one
        // operation stays one delivery without a claim and a confirm of its own.
        if let Some(receipt) = &item.receipt {
            record.attempt = 1;
            record.last_fence = 1;
            record.revision = 1;
            record.state = crate::scheduler::core::outbox::records::OutboxState::Delivered {
                receipt: receipt.clone(),
            };
        }
        new_outbox.push(id.clone());
        next.outbox.insert(id, record);
    }

    let graph_delta = u64::from(!plan.task_creates.is_empty());
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
        // The operation closes inside the *same* commit as its
        // business effect. The two used to be two whole-aggregate compare-and-
        // swaps (`OperationEvent::Commit`, then a second plan that carried only
        // `OperationEvent::Complete`), and nothing observed the state between
        // them: the close committed the flag and nothing else, the effect
        // commit having written the new operations and outbox records in the
        // first place. Applying the pair in order here leaves the aggregate in
        // exactly the state the two commits left — the operation `Completed`,
        // its successors and records `Pending` in the recovery index, which is
        // what the next scan wakes either way.
        //
        // `Committed` stays a persisted state: the V2 tags decode it and
        // `RecoveryPlanner::FinalizeCommit` still finishes the aggregates a
        // writer from before this fold left there. No operation produces it any more.
        let committed = operation
            .evolve(OperationEvent::Commit {
                claim: source.claim.clone(),
                commit: token.clone(),
            })
            .map_err(|_| CommitError::InvalidOperationState(operation.state.clone()))?;
        let completed = committed
            .evolve(OperationEvent::Complete {
                commit: token.clone(),
            })
            .map_err(|_| CommitError::InvalidOperationState(committed.state.clone()))?;
        next.operations.insert(source.id.clone(), completed);
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
    snapshot: &ProcessSnapshot,
    plan: &TransitionPlan,
) -> Result<(), CommitError> {
    if snapshot.version != plan.expected {
        return Err(CommitError::VersionConflict {
            expected: plan.expected.clone(),
            actual: snapshot.version.clone(),
        });
    }
    let created = plan
        .task_creates
        .iter()
        .map(|item| item.task.clone())
        .collect::<HashSet<_>>();
    for create in &plan.task_creates {
        if snapshot.tasks.contains_key(&create.task) {
            return Err(CommitError::TaskAlreadyExists(create.task.clone()));
        }
        if let Some(parent) = &create.parent
            && !snapshot.tasks.contains_key(parent)
            && !created.contains(parent)
        {
            return Err(CommitError::TaskNotFound(parent.clone()));
        }
        for predecessor in &create.predecessors {
            if !snapshot.tasks.contains_key(predecessor) && !created.contains(predecessor) {
                return Err(CommitError::TaskNotFound(predecessor.clone()));
            }
        }
        if snapshot
            .tasks
            .keys()
            .any(|id| id.node == create.task.node && id.generation >= create.task.generation)
        {
            return Err(CommitError::GenerationNotMonotonic(create.task.clone()));
        }
    }
    for change in &plan.state_changes {
        let current = snapshot
            .tasks
            .get(&change.task)
            .ok_or_else(|| CommitError::TaskNotFound(change.task.clone()))?;
        if current.revision != change.expected_revision || current.state != change.from {
            return Err(CommitError::TaskConflict {
                task: change.task.clone(),
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
        if !snapshot.tasks.contains_key(&write.scope) && !created.contains(&write.scope) {
            return Err(CommitError::TaskNotFound(write.scope.clone()));
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

pub(super) fn operation_order(
    left: &crate::scheduler::core::model::operation::OperationId,
    right: &crate::scheduler::core::model::operation::OperationId,
) -> std::cmp::Ordering {
    left.process_id
        .cmp(&right.process_id)
        .then_with(|| left.task.node.cmp(&right.task.node))
        .then_with(|| left.task.generation.cmp(&right.task.generation))
        .then_with(|| left.sequence.cmp(&right.sequence))
}
