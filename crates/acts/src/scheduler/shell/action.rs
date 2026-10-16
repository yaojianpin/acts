use super::*;

/// The durable target of a `Cancel`/`Back` rewind, written by the deciding
/// commit for the `ReenterTask` operation its batch creates: `Back` names an
/// earlier step (the client's `to`), `Cancel` the closed execution itself.
/// A private fact (the `__` prefix keeps it out of the expression space and
/// the read model), like every other engine fact the hops address each other
/// with.
pub(super) const REENTER_TARGET_VAR: &str = "__reenter_target";

/// The durable identity of one client action: the execution it targets and
/// the operation that carries its intent. Pinned by the first attempt so
/// every retry re-drives the same operation instead of re-deriving ids from a
/// snapshot the action's own commit moved.
pub(super) struct ActionIntent {
    pub(super) task: crate::scheduler::core::model::task_graph::TaskId,
    pub(super) operation: OperationId,
}

/// What a retried action's pinned intent says about the previous attempt.
pub(super) enum ActionResume {
    /// The intent's commit is durable: the action applied.
    Applied,
    /// The intent is still open (or its ownership was released): re-drive it.
    Retry,
}

/// The private variable a durable action intent pins its decision in, keyed by
/// the intent operation's sequence so a later action on the same execution
/// cannot be read in its place. A `__` fact: out of the expression space and
/// the read model.
pub(super) fn action_fact_var(sequence: u64) -> String {
    format!("__action@{sequence}")
}

/// The action a durable intent carries, as the JSON text its hop replays.
pub(super) fn action_fact(
    action: &crate::scheduler::core::commit::command::ActionKind,
    inputs: &[(String, String)],
) -> String {
    use crate::scheduler::core::commit::command::ActionKind as A;
    let kind = match action {
        A::Complete => serde_json::json!({ "action": "complete" }),
        A::Submit => serde_json::json!({ "action": "submit" }),
        A::Skip => serde_json::json!({ "action": "skip" }),
        A::Cancel => serde_json::json!({ "action": "cancel" }),
        A::Abort => serde_json::json!({ "action": "abort" }),
        A::Back => serde_json::json!({ "action": "back" }),
        A::Fail { code, message } => serde_json::json!({
            "action": "fail",
            "code": code,
            "message": message,
        }),
    };
    serde_json::json!({
        "kind": kind,
        "inputs": inputs
            .iter()
            .map(|(name, value)| serde_json::json!([name, value]))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// Decode a durable action fact back into the decision it records.
pub(super) fn decode_action_fact(
    text: &str,
) -> Result<(
    crate::scheduler::core::commit::command::ActionKind,
    Vec<(String, String)>,
)> {
    let fact: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| runtime(format!("corrupt action fact: {error}")))?;
    let kind = &fact["kind"];
    let action = match kind["action"].as_str().unwrap_or_default() {
        "complete" => crate::scheduler::core::commit::command::ActionKind::Complete,
        "submit" => crate::scheduler::core::commit::command::ActionKind::Submit,
        "skip" => crate::scheduler::core::commit::command::ActionKind::Skip,
        "cancel" => crate::scheduler::core::commit::command::ActionKind::Cancel,
        "abort" => crate::scheduler::core::commit::command::ActionKind::Abort,
        "back" => crate::scheduler::core::commit::command::ActionKind::Back,
        "fail" => crate::scheduler::core::commit::command::ActionKind::Fail {
            code: kind["code"].as_str().unwrap_or_default().to_string(),
            message: kind["message"].as_str().unwrap_or_default().to_string(),
        },
        other => return Err(runtime(format!("corrupt action fact: action '{other}'"))),
    };
    let inputs = fact["inputs"]
        .as_array()
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_string(),
                        pair.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok((action, inputs))
}
