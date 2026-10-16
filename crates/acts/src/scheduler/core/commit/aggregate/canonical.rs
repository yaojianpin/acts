use super::*;

fn task_state_tag(state: TaskState) -> &'static str {
    match state {
        TaskState::Created => "Created",
        TaskState::Ready => "Ready",
        TaskState::Running => "Running",
        TaskState::Waiting => "Waiting",
        TaskState::Completed => "Completed",
        TaskState::Submitted => "Submitted",
        TaskState::Backed => "Backed",
        TaskState::Cancelled => "Cancelled",
        TaskState::Failed => "Failed",
        TaskState::Aborted => "Aborted",
        TaskState::Skipped => "Skipped",
        TaskState::Removed => "Removed",
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

pub(crate) fn canonical_process_bytes(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, CommitError> {
    // Canonical, unambiguous change token over every persisted field. Strings
    // are length-prefixed so user data containing separators cannot collide.
    fn field(output: &mut String, value: &str) {
        output.push_str(&value.len().to_string());
        output.push(':');
        output.push_str(value);
    }
    fn task_id(output: &mut String, id: &TaskId) {
        field(output, &id.node);
        field(output, &id.generation.to_string());
    }

    let mut output = String::new();
    field(&mut output, "aggregate-v1");
    field(&mut output, &snapshot.process_id);
    field(&mut output, &snapshot.version.graph.to_string());
    field(&mut output, &snapshot.version.process.to_string());
    field(&mut output, &snapshot.version.operations.to_string());

    let mut tasks = snapshot.tasks.iter().collect::<Vec<_>>();
    tasks.sort_by_key(|(id, _)| *id);
    for (id, value) in tasks {
        field(&mut output, "execution");
        task_id(&mut output, id);
        field(&mut output, value.model_node.as_str());
        match &value.parent {
            Some(parent) => {
                field(&mut output, "parent");
                task_id(&mut output, parent);
            }
            None => field(&mut output, "no-parent"),
        }
        let mut predecessors = value.predecessors.clone();
        predecessors.sort();
        field(&mut output, &predecessors.len().to_string());
        for predecessor in predecessors {
            task_id(&mut output, &predecessor);
        }
        field(&mut output, task_state_tag(value.state));
        field(&mut output, &value.revision.to_string());
    }
    for ((id, name), value) in &snapshot.variables {
        field(&mut output, "variable");
        task_id(&mut output, id);
        field(&mut output, name);
        field(&mut output, value);
    }
    let mut operations = snapshot.operations.values().collect::<Vec<_>>();
    operations.sort_by(|left, right| operation_order(&left.id, &right.id));
    for operation in operations {
        field(&mut output, "operation");
        field(&mut output, &operation.id.process_id);
        field(&mut output, &operation.id.node);
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

pub(super) fn deterministic_digest(snapshot: &ProcessSnapshot) -> Result<String, CommitError> {
    Ok(sha256_hex(&canonical_process_bytes(snapshot)?))
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
