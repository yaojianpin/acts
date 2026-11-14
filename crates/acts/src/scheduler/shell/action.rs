use super::*;

/// The durable target of a `Cancel`/`Back` rewind, written by the deciding
/// commit for the `ReenterTask` operation its batch creates: `Back` names an
/// earlier step (the client's `to`), `Cancel` the closed execution itself.
/// A private fact (the `#` prefix keeps it out of the expression space and
/// the read model), like every other engine fact the operations address each other
/// with.
pub(super) const REENTER_TARGET_VAR: &str = "#reenter_target";

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
/// cannot be read in its place. A `#` fact: out of the expression space and
/// the read model.
pub(super) fn action_fact_var(sequence: u64) -> String {
    format!("#action@{sequence}")
}

/// The durable spec of a pushed act, on the pushed execution's own
/// scope. The legacy `dispatch_act` grew a dynamic tree node for the act; the
/// kernel's model is immutable for the life of a run, so the act rides the
/// execution instead, and every model lookup for the node falls back to it
/// (`step_uses`/`step_effect`).
pub(super) const PUSH_ACT_VAR: &str = "#act";

/// The pushed act one execution was created for: the client inputs a `Push`
/// carried, reduced to what an act's start and messages read off its model
/// step.
#[derive(Clone, Debug)]
pub(super) struct PushedAct {
    pub(super) uses: String,
    pub(super) name: String,
    pub(super) params: serde_json::Value,
    pub(super) options: serde_json::Value,
    pub(super) exposes: Vec<crate::Variant>,
    pub(super) vars: Vec<crate::Variant>,
    /// The pushed human act's owner declaration. The act resolves it at its
    /// own start through the same assignment path a declared leaf uses.
    pub(super) assign: Option<crate::model::Assign>,
}

impl PushedAct {
    /// Reduce the push inputs to the act's facts. `uses` is required — the
    /// wire mapping refuses its absence before anything durable exists, and
    /// this is the replay-side twin of that guard.
    pub(super) fn from_inputs(inputs: &[(String, String)]) -> Result<Self> {
        fn list(name: &str, raw: Option<String>) -> Result<Vec<crate::Variant>> {
            match raw {
                Some(raw) => serde_json::from_str(&raw).map_err(|error| {
                    runtime(format!("push '{name}' is not a variant list: {error}"))
                }),
                None => Ok(Vec::new()),
            }
        }
        let get = |name: &str| {
            inputs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let uses = get("uses").unwrap_or_default();
        if uses.trim().is_empty() {
            return Err(runtime("cannot find 'uses' in act"));
        }
        let params = match get("params") {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|error| runtime(format!("push 'params' is not json: {error}")))?,
            None => serde_json::Value::Null,
        };
        let options = match get("options") {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|error| runtime(format!("push 'options' is not json: {error}")))?,
            None => serde_json::Value::Null,
        };
        let assign = match get("assign") {
            Some(raw) if !raw.trim().is_empty() && raw.trim() != "null" => Some(
                serde_json::from_str::<crate::model::Assign>(&raw)
                    .map_err(|error| runtime(format!("push 'assign' is not an owner: {error}")))?,
            ),
            _ => None,
        };
        Ok(Self {
            uses,
            name: get("name").unwrap_or_default(),
            params,
            options,
            exposes: list("exposes", get("exposes"))?,
            vars: list("vars", get("vars"))?,
            assign,
        })
    }

    pub(super) fn fact(&self) -> String {
        serde_json::json!({
            "uses": self.uses,
            "name": self.name,
            "params": self.params,
            "options": self.options,
            "exposes": self.exposes,
            "vars": self.vars,
            "assign": self.assign,
        })
        .to_string()
    }

    fn decode(text: &str) -> Result<Self> {
        let fact: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| runtime(format!("corrupt push fact: {error}")))?;
        let uses = fact["uses"].as_str().unwrap_or_default().to_string();
        if uses.trim().is_empty() {
            return Err(runtime("corrupt push fact: no 'uses'"));
        }
        let variant_list = |value: &serde_json::Value| {
            serde_json::from_value::<Vec<crate::Variant>>(value.clone())
                .map_err(|error| runtime(format!("corrupt push fact: {error}")))
        };
        Ok(Self {
            uses,
            name: fact["name"].as_str().unwrap_or_default().to_string(),
            params: fact["params"].clone(),
            options: fact["options"].clone(),
            exposes: variant_list(&fact["exposes"])?,
            vars: variant_list(&fact["vars"])?,
            assign: serde_json::from_value(fact["assign"].clone()).map_err(|error| {
                runtime(format!("corrupt push fact: invalid 'assign': {error}"))
            })?,
        })
    }

    /// The model step the headless scope resolves this act through: the
    /// kernel's model is immutable, so the pushed act's node is synthesized
    /// from its own fact wherever the declaring model lacks it.
    pub(super) fn step(&self, node: &str) -> crate::model::Step {
        crate::model::Step {
            id: node.to_string(),
            name: self.name.clone(),
            uses: Some(self.uses.clone()),
            params: self.params.clone(),
            options: serde_json::from_value(self.options.clone()).unwrap_or_default(),
            exposes: self.exposes.clone(),
            vars: self.vars.clone(),
            assign: self.assign.clone(),
            ..Default::default()
        }
    }
}

/// The pushed act one execution was created for, if any.
pub(super) fn pushed_act_of(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
) -> Result<Option<PushedAct>> {
    snapshot
        .variables
        .get(&(task_id.clone(), PUSH_ACT_VAR.to_string()))
        .map(|raw| PushedAct::decode(raw))
        .transpose()
}

/// The next pushed node's id under `task_id`: `<target>#push<k>`, unique
/// against every execution the run ever held, so a replay against the same
/// snapshot mints the same id (idempotency) and two pushes never collide.
/// A plain push and the leaves of an approver chain share the counter.
pub(super) fn next_push_node(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
) -> String {
    let prefix = format!("{}#push", task_id.node.as_str());
    let nth = snapshot
        .tasks
        .keys()
        .filter(|existing| existing.node.as_str().starts_with(&prefix))
        .count()
        + 1;
    format!("{prefix}{nth}")
}

/// The pending approver chain (the sequential added approvers) one execution
/// drives: the owners still to admit — one pushed act at a time, in the
/// order the push fixed (`before` runs the added approvers ahead of the
/// owner's turn, `after` behind it) — and the params every leaf of the
/// chain carries. The variable lives on the target's scope; each settled
/// leaf pops its front
/// ([`ApplyAction::approver_push_plan`](../operation/action/struct.ApplyAction.html)).
pub(super) const APPROVER_CHAIN_VAR: &str = "#approver_chain";

/// The durable approver chain of one added-approvers push.
#[derive(Clone, Debug)]
pub(super) struct ApproverChain {
    pub(super) owners: Vec<String>,
    pub(super) params: serde_json::Value,
}

impl ApproverChain {
    pub(super) fn fact(&self) -> String {
        serde_json::json!({ "owners": self.owners, "params": self.params }).to_string()
    }

    pub(super) fn decode(text: &str) -> Result<Self> {
        let fact: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| runtime(format!("corrupt approver chain: {error}")))?;
        let owners = fact["owners"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            owners,
            params: fact["params"].clone(),
        })
    }
}

/// The approver chain one execution still drives, if any.
pub(super) fn approver_chain_of(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
) -> Result<Option<ApproverChain>> {
    snapshot
        .variables
        .get(&(task_id.clone(), APPROVER_CHAIN_VAR.to_string()))
        .map(|raw| ApproverChain::decode(raw))
        .transpose()
}

/// The added approvers a client push names, against a single-owner act:
/// `owners` is a non-empty JSON array of user ids and `mode` fixes the order
/// — `before` (the added steps ahead of the owner's) or `after` (the owner's
/// ahead, the default). Absent `owners` is the plain push whose act the
/// client spells out itself.
#[derive(Clone, Debug)]
pub(crate) struct ApproverRequest {
    pub(super) owners: Vec<String>,
    pub(super) before: bool,
    pub(super) params: serde_json::Value,
}

impl ApproverRequest {
    pub(crate) fn from_inputs(inputs: &[(String, String)]) -> Result<Option<Self>> {
        let get = |name: &str| {
            inputs
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let Some(raw) = get("owners").filter(|raw| !raw.trim().is_empty()) else {
            return Ok(None);
        };
        let owners: Vec<String> = serde_json::from_str(&raw)
            .map_err(|error| runtime(format!("push 'owners' is not a user list: {error}")))?;
        if owners.is_empty() || owners.iter().any(|owner| owner.trim().is_empty()) {
            return Err(runtime("push 'owners' names no approver"));
        }
        let before = match get("mode").unwrap_or_default().as_str() {
            "" | "after" => false,
            "before" => true,
            other => {
                return Err(runtime(format!(
                    "push 'mode' is '{other}': name 'before' or 'after'"
                )));
            }
        };
        let params = match get("params").filter(|raw| !raw.trim().is_empty()) {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|error| runtime(format!("push 'params' is not json: {error}")))?,
            None => serde_json::Value::Null,
        };
        Ok(Some(Self {
            owners,
            before,
            params,
        }))
    }
}

/// The action a durable intent carries, as the JSON text its operation replays.
/// `subject` is the authenticated ACL subject (the executor's fill): the audit
/// fields and the authorization gate use it, and it is persisted because a
/// replayed decision must announce the same audit trail as the original call.
/// `actor` is the business actor the client declared (`actor` option): `$actor`,
/// `$previous_approver` and the payload attribution read it. Neither is ever
/// read from the client's person fields.
pub(super) fn action_fact(
    action: &crate::scheduler::core::commit::command::ActionKind,
    inputs: &[(String, String)],
    subject: Option<&str>,
    actor: Option<&str>,
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
        A::Push => serde_json::json!({ "action": "push" }),
        A::Remove => serde_json::json!({ "action": "remove" }),
        A::Transfer => serde_json::json!({ "action": "transfer" }),
        A::Delegate => serde_json::json!({ "action": "delegate" }),
        A::Undelegate => serde_json::json!({ "action": "undelegate" }),
        A::Remind => serde_json::json!({ "action": "remind" }),
    };
    serde_json::json!({
        "kind": kind,
        "subject": subject,
        "actor": actor,
        "inputs": inputs
            .iter()
            .map(|(name, value)| serde_json::json!([name, value]))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// A decoded durable action fact: the action, its inputs, the authenticated
/// ACL subject and the business actor (both absent on facts an older kernel
/// wrote).
pub(super) type DecodedActionFact = (
    crate::scheduler::core::commit::command::ActionKind,
    Vec<(String, String)>,
    Option<String>,
    Option<String>,
);

/// Decode a durable action fact back into the decision it records.
pub(super) fn decode_action_fact(text: &str) -> Result<DecodedActionFact> {
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
        "push" => crate::scheduler::core::commit::command::ActionKind::Push,
        "remove" => crate::scheduler::core::commit::command::ActionKind::Remove,
        "transfer" => crate::scheduler::core::commit::command::ActionKind::Transfer,
        "delegate" => crate::scheduler::core::commit::command::ActionKind::Delegate,
        "undelegate" => crate::scheduler::core::commit::command::ActionKind::Undelegate,
        "remind" => crate::scheduler::core::commit::command::ActionKind::Remind,
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
    let subject = fact["subject"].as_str().map(str::to_string);
    let actor = fact["actor"].as_str().map(str::to_string);
    Ok((action, inputs, subject, actor))
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
            ActionKind::Push,
            ActionKind::Remove,
            ActionKind::Transfer,
            ActionKind::Delegate,
            ActionKind::Undelegate,
        ] {
            let text = action_fact(&action, &inputs, Some("alice"), Some("u1"));
            let (decoded, decoded_inputs, subject, actor) = decode_action_fact(&text).unwrap();
            assert_eq!(decoded, action);
            assert_eq!(decoded_inputs, inputs);
            assert_eq!(subject.as_deref(), Some("alice"));
            assert_eq!(actor.as_deref(), Some("u1"));
            // An unattributed fact (the engine origin) decodes without either.
            let text = action_fact(&action, &inputs, None, None);
            let (_, _, subject, actor) = decode_action_fact(&text).unwrap();
            assert_eq!(subject, None);
            assert_eq!(actor, None);
        }
        assert_eq!(action_fact_var(7), "#action@7");
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
        let (action, inputs, subject, actor) = decode_action_fact(text).unwrap();
        assert_eq!(subject, None);
        assert_eq!(actor, None);
        assert_eq!(
            action,
            ActionKind::Fail {
                code: "E".into(),
                message: "".into(),
            }
        );
        assert_eq!(inputs, vec![("ok".to_string(), "1".to_string())]);

        let (_, inputs, subject, actor) =
            decode_action_fact(r#"{"kind":{"action":"skip"},"inputs":7}"#).unwrap();
        assert!(inputs.is_empty());
        assert_eq!(subject, None);
        assert_eq!(actor, None);
    }
}
