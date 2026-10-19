use super::*;

/// The durable target of a `Cancel`/`Back` rewind, written by the deciding
/// commit for the `ReenterTask` operation its batch creates: `Back` names an
/// earlier step (the client's `to`), `Cancel` the closed execution itself.
/// A private fact (the `__` prefix keeps it out of the expression space and
/// the read model), like every other engine fact the operations address each other
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

/// The action a durable intent carries, as the JSON text its operation replays.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::command::ActionKind;

    /// The durable fact is the action's own spelling: every kind and every
    /// input pair survives the JSON round trip the operation replays.
    #[test]
    fn a_fact_round_trips_every_action_and_its_inputs() {
        let inputs = vec![
            ("k".to_string(), "\"v\"".to_string()),
            ("n".to_string(), "1".to_string()),
        ];
        for action in [
            ActionKind::Complete,
            ActionKind::Submit,
            ActionKind::Skip,
            ActionKind::Cancel,
            ActionKind::Abort,
            ActionKind::Back,
            ActionKind::Fail {
                code: "E1".into(),
                message: "boom".into(),
            },
        ] {
            let text = action_fact(&action, &inputs);
            let (decoded, decoded_inputs) = decode_action_fact(&text).unwrap();
            assert_eq!(decoded, action);
            assert_eq!(decoded_inputs, inputs);
        }
        assert_eq!(action_fact_var(7), "__action@7");
    }

    /// A fact the engine cannot read back is an engine error naming the
    /// corruption, never a silent default decision.
    #[test]
    fn a_corrupt_fact_is_an_engine_error() {
        let error = decode_action_fact("not json").unwrap_err();
        assert!(format!("{error}").contains("corrupt action fact"));
        let error = decode_action_fact(r#"{"kind":{"action":"explode"}}"#).unwrap_err();
        assert!(format!("{error}").contains("action 'explode'"), "{error}");
        // A missing action is spelled empty and still refused.
        let error = decode_action_fact("{}").unwrap_err();
        assert!(format!("{error}").contains("action ''"), "{error}");
    }

    /// Non-array inputs and malformed pairs are skipped: a fact written by an
    /// older kernel still decodes its action, and a pair that cannot name both
    /// sides carries no input.
    #[test]
    fn malformed_input_pairs_are_skipped() {
        let text = r#"{"kind":{"action":"fail","code":"E"},"inputs":["x",["only-name"],[1,2],["ok","1"]]}"#;
        let (action, inputs) = decode_action_fact(text).unwrap();
        assert_eq!(
            action,
            ActionKind::Fail {
                code: "E".into(),
                message: "".into(),
            }
        );
        assert_eq!(inputs, vec![("ok".to_string(), "1".to_string())]);

        let (_, inputs) = decode_action_fact(r#"{"kind":{"action":"skip"},"inputs":7}"#).unwrap();
        assert!(inputs.is_empty());
    }
}
