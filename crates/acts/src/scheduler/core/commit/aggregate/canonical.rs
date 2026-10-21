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
        crate::scheduler::core::model::operation::OperationKind::ReturnToParent => "ReturnToParent",
        crate::scheduler::core::model::operation::OperationKind::DeliverMessage => "DeliverMessage",
        crate::scheduler::core::model::operation::OperationKind::FireTrigger => "FireTrigger",
        crate::scheduler::core::model::operation::OperationKind::RunPostCommit => "RunPostCommit",
    }
}

/// Append `value`'s decimal digits to `output`.
fn push_decimal(output: &mut Vec<u8>, mut value: u64) {
    if value == 0 {
        output.push(b'0');
        return;
    }
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    while value > 0 {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    output.extend_from_slice(&digits[start..]);
}

/// Append a signed decimal (the claim's lease instant is the only one).
fn push_signed(output: &mut Vec<u8>, value: i64) {
    if value < 0 {
        output.push(b'-');
        push_decimal(output, value.unsigned_abs());
    } else {
        push_decimal(output, value as u64);
    }
}

/// Append a length-prefixed field: `<len>:<value>`. Strings stay unambiguous
/// even when user data contains separators, because the length is read first.
fn field(output: &mut Vec<u8>, value: &[u8]) {
    push_decimal(output, value.len() as u64);
    output.push(b':');
    output.extend_from_slice(value);
}

fn field_str(output: &mut Vec<u8>, value: &str) {
    field(output, value.as_bytes());
}

fn field_u64(output: &mut Vec<u8>, value: u64) {
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    let mut remaining = value;
    if remaining == 0 {
        start -= 1;
        digits[start] = b'0';
    } else {
        while remaining > 0 {
            start -= 1;
            digits[start] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
        }
    }
    field(output, &digits[start..]);
}

fn task_id_field(output: &mut Vec<u8>, id: &TaskId) {
    field_str(output, id.node.as_str());
    field_u64(output, id.generation);
}

/// Append `value` the way `str::escape_debug` renders it, wrapped in quotes —
/// the exact text the canonical form has always carried for free-form strings.
fn quoted(output: &mut Vec<u8>, value: &str) {
    output.push(b'"');
    for escaped in value.escape_debug() {
        let mut buf = [0u8; 4];
        output.extend_from_slice(escaped.encode_utf8(&mut buf).as_bytes());
    }
    output.push(b'"');
}

fn claim_token(output: &mut Vec<u8>, claim: &crate::scheduler::core::model::operation::ClaimToken) {
    output.extend_from_slice(b"ClaimToken { owner: ");
    quoted(output, &claim.owner);
    output.extend_from_slice(b", fence: ");
    push_decimal(output, claim.fence);
    output.extend_from_slice(b", lease_until_ms: ");
    push_signed(output, claim.lease_until_ms);
    output.extend_from_slice(b", heartbeat_revision: ");
    push_decimal(output, claim.heartbeat_revision);
    output.extend_from_slice(b" }");
}

fn commit_token(output: &mut Vec<u8>, commit: &CommitToken) {
    output.extend_from_slice(b"CommitToken { process_version: ");
    push_decimal(output, commit.process_version);
    output.extend_from_slice(b", digest: ");
    quoted(output, &commit.digest);
    output.extend_from_slice(b" }");
}

fn operation_state(output: &mut Vec<u8>, state: &OperationState) {
    match state {
        OperationState::Pending => output.extend_from_slice(b"Pending"),
        OperationState::Claimed(claim) => {
            output.extend_from_slice(b"Claimed(");
            claim_token(output, claim);
            output.push(b')');
        }
        OperationState::CommitPrepared(claim) => {
            output.extend_from_slice(b"CommitPrepared(");
            claim_token(output, claim);
            output.push(b')');
        }
        OperationState::Committed(commit) => {
            output.extend_from_slice(b"Committed(");
            commit_token(output, commit);
            output.push(b')');
        }
        OperationState::RetryScheduled { attempt } => {
            output.extend_from_slice(b"RetryScheduled { attempt: ");
            push_decimal(output, u64::from(*attempt));
            output.extend_from_slice(b" }");
        }
        OperationState::EffectUnknown { attempt } => {
            output.extend_from_slice(b"EffectUnknown { attempt: ");
            push_decimal(output, u64::from(*attempt));
            output.extend_from_slice(b" }");
        }
        OperationState::Quarantined { reason } => {
            output.extend_from_slice(b"Quarantined { reason: ");
            quoted(output, reason);
            output.extend_from_slice(b" }");
        }
        OperationState::Superseded { by_generation } => {
            output.extend_from_slice(b"Superseded { by_generation: ");
            push_decimal(output, *by_generation);
            output.extend_from_slice(b" }");
        }
        OperationState::Completed(commit) => {
            output.extend_from_slice(b"Completed(");
            commit_token(output, commit);
            output.push(b')');
        }
    }
}

pub(crate) fn canonical_process_bytes(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, CommitError> {
    // Canonical, unambiguous change token over every persisted field. Strings
    // are length-prefixed so user data containing separators cannot collide.
    // The bytes are appended straight into one buffer: a commit runs this over
    // the whole aggregate, and per-field `format!`/`to_string` allocation made
    // that path dominate an unoptimized build.
    let mut output = Vec::new();
    field_str(&mut output, "aggregate-v1");
    field_str(&mut output, &snapshot.process_id);
    field_u64(&mut output, snapshot.version.graph);
    field_u64(&mut output, snapshot.version.process);
    field_u64(&mut output, snapshot.version.operations);

    let mut tasks = snapshot.tasks.iter().collect::<Vec<_>>();
    tasks.sort_by_key(|(id, _)| *id);
    for (id, value) in tasks {
        field_str(&mut output, "execution");
        task_id_field(&mut output, id);
        field_str(&mut output, value.model_node.as_str());
        match &value.parent {
            Some(parent) => {
                field_str(&mut output, "parent");
                task_id_field(&mut output, parent);
            }
            None => field_str(&mut output, "no-parent"),
        }
        let mut predecessors = value.predecessors.clone();
        predecessors.sort();
        field_u64(&mut output, predecessors.len() as u64);
        for predecessor in predecessors {
            task_id_field(&mut output, &predecessor);
        }
        field_str(&mut output, task_state_tag(value.state));
        field_u64(&mut output, value.revision);
    }
    for ((id, name), value) in &snapshot.variables {
        field_str(&mut output, "variable");
        task_id_field(&mut output, id);
        field_str(&mut output, name);
        field_str(&mut output, value);
    }
    let mut operations = snapshot.operations.values().collect::<Vec<_>>();
    operations.sort_by(|left, right| operation_order(&left.id, &right.id));
    let mut state = Vec::new();
    for operation in operations {
        field_str(&mut output, "operation");
        field_str(&mut output, &operation.id.process_id);
        field_str(&mut output, operation.id.task.node.as_str());
        field_u64(&mut output, operation.id.task.generation);
        field_u64(&mut output, operation.id.sequence);
        field_str(&mut output, operation_kind_tag(operation.kind));
        state.clear();
        operation_state(&mut state, &operation.state);
        field(&mut output, &state);
        field_u64(&mut output, operation.revision);
    }
    let mut outbox = snapshot.outbox.values().collect::<Vec<_>>();
    outbox.sort_by(|left, right| left.id.cmp(&right.id));
    for record in outbox {
        let encoded = crate::scheduler::core::outbox::outbox_codec::OutboxCodec::encode(record)
            .map_err(|error| CommitError::OutboxEncoding(error.to_string()))?;
        field_str(&mut output, "outbox");
        field_str(&mut output, &sha256_hex(&encoded));
    }
    Ok(output)
}

pub(super) fn deterministic_digest(snapshot: &ProcessSnapshot) -> Result<String, CommitError> {
    Ok(sha256_hex(&canonical_process_bytes(snapshot)?))
}

pub(crate) fn sha256_hex(input: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    // Two hex digits per byte from a table: `write!` per byte formats through
    // `core::fmt` and is one of the hottest paths of a commit (every aggregate
    // envelope and every outbox record is hashed), where its cost is pure
    // overhead in an unoptimized build.
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
