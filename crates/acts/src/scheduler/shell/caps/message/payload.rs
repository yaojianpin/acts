//! The message-payload half of the message capability: building the act and
//! task payloads an operation announces.
//!
//! Every builder is a pure function over the durable snapshot — no host
//! state — so the operation modules (start, action, propagate, cascade) call
//! them directly and the transport half ([`super::MessageForwarder`]) maps
//! the durable record onto the engine's `Message` unchanged.

use crate::{
    NodeKind, Result,
    scheduler::{
        core::{
            commit::aggregate::ProcessSnapshot,
            model::{
                model_graph::NodeId,
                task_graph::TaskId,
                transition_plan::{OperationIntent, OutboxIntent, VariableWrite},
            },
        },
        shell::{
            caps::assign::{Assign, Assignment},
            errors::runtime,
            facts::{ActFacts, input_vars, message_outputs, node_facts},
            find_step_in_model, model_of,
        },
    },
};

/// The `(uses, raw params, node name, model id)` of a `uses` step node,
/// from the durable model — the act message's and the effect seam's
/// package facts.
pub(crate) fn step_uses(
    snapshot: &ProcessSnapshot,
    task_id: &TaskId,
    model_node: &crate::scheduler::core::model::model_graph::NodeId,
) -> Result<Option<(String, serde_json::Value, String, String)>> {
    let Some(model) = model_of(snapshot)? else {
        return Ok(None);
    };
    let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
        // A pushed act is not in the model: its facts ride the
        // execution's scope (see `push_plan`).
        let Some(act) = crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)? else {
            return Ok(None);
        };
        return Ok(Some((act.uses, act.params, act.name, model.id.clone())));
    };
    let Some(uses) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some((
        uses,
        step.params.clone(),
        step.name.clone(),
        model.id.clone(),
    )))
}

/// The act message payload of one `uses` step: the fact set the driver's
/// transport maps onto the engine's `Message`, faithful to the legacy
/// `Task::create_message` — the node level (`act`), the resolved params and
/// options, the step and model inputs, the scope inputs the message
/// carries, then the operation's own outputs or its error. `facts` are the
/// start operation's pinned resolution
/// (absent for a host without a runner or an aggregate seeded before it);
/// `outputs` are the operation's own scope writes (`__`-private facts and the
/// error variables excluded); `error` is the failure the operation decided.
#[allow(clippy::too_many_arguments)]
fn act_message_payload(
    snapshot: &ProcessSnapshot,
    task_id: &TaskId,
    node_id: &str,
    facts: Option<&ActFacts>,
    outputs: Option<&[VariableWrite]>,
    error: Option<(&str, &str)>,
) -> Result<Option<serde_json::Value>> {
    let node = NodeId::parse(node_id).map_err(runtime)?;
    let Some((uses, raw_params, name, mid)) = step_uses(snapshot, task_id, &node)? else {
        return Ok(None);
    };
    // The resolved facts when the start operation pinned them, the raw
    // declaration otherwise (`params` before a runner ever saw the step).
    let params = facts
        .map(|facts| facts.params.clone())
        .unwrap_or(raw_params);
    let options = facts
        .map(|facts| facts.options.clone())
        .unwrap_or(serde_json::Value::Null);
    // The assignment projection: the persisted task state is the only
    // source of these fields, never a client's inputs or options. It is
    // merged into the payload after the inputs are built (see
    // `inject_payload_assignment`).
    let assignment = Assignment::of(snapshot, task_id)?;

    let mut inputs = input_vars(snapshot, task_id);
    // The act folds into its step node in the kernel, so the step the
    // legacy message pointed at is the node itself.
    inputs.insert(
        crate::utils::consts::STEP_KEY.to_string(),
        serde_json::json!({
            crate::utils::consts::STEP_NODE_ID: node_id,
            crate::utils::consts::STEP_NODE_NAME: name,
            // The instance's external id — the wire since C5 (the legacy
            // `$step.task_id` was the legacy task's own id, not the node).
            crate::utils::consts::STEP_TASK_ID: task_id.wire(&snapshot.process_id),
        }),
    );
    if let Some(model) = model_of(snapshot)? {
        inputs.insert(
            crate::utils::consts::WORKFLOW_MODEL_KEY.to_string(),
            serde_json::json!({
                "id": model.id,
                "name": model.name,
                "options": model.options,
            }),
        );
    }
    inputs.insert(crate::utils::consts::ACT_PARAMS_KEY.to_string(), params);
    inputs.insert(crate::utils::consts::ACT_OPTIONS_KEY.to_string(), options);

    let mut payload = serde_json::json!({
        "tid": task_id.wire(&snapshot.process_id),
        "nid": node_id,
        "name": name,
        // The level is explicit since C6: the act payload used to leave
        // `kind` to the consumer's "act" default. The word is unchanged.
        "kind": NodeKind::Act.to_string(),
        "uses": uses,
        "mid": mid,
        "inputs": serde_json::Value::Object(inputs),
    });
    if let Some(assignment) = &assignment {
        Assign::inject_payload_assignment(&mut payload, assignment);
    }
    if let Some((code, message)) = error {
        payload["error"] = serde_json::json!({ "code": code, "message": message });
    } else if let Some(outputs) = outputs {
        payload["outputs"] = message_outputs(outputs, task_id);
    }
    Ok(Some(payload))
}

/// The task-level message payload of one execution *without* a package: the
/// fact set the driver's transport maps onto the engine's `Message`. Every
/// execution announces its terminal states — skipped, completed, aborted,
/// failed and the rest — whatever its level, so a client can rebuild the
/// run's flow from the message stream alone: a package-less step and the
/// workflow root announce like the legacy `on_task` did, and a branch
/// announces too (the legacy tree kept branches silent; the kernel's
/// message stream is the trace, so a skipped or completed branch is a
/// state the client must see — its `kind` tells the two levels apart).
fn task_message_payload(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
    node: &str,
    outputs: Option<&[VariableWrite]>,
    error: Option<(&str, &str)>,
) -> Result<Option<serde_json::Value>> {
    let Some(model) = model_of(snapshot)? else {
        return Ok(None);
    };
    let facts = node_facts(&model, node);
    let mut inputs = input_vars(snapshot, task_id);
    // The workflow model rides every legacy message; a package-less task
    // carries no params and no `step` link of its own.
    inputs.insert(
        crate::utils::consts::WORKFLOW_MODEL_KEY.to_string(),
        serde_json::json!({
            "id": model.id,
            "name": model.name,
            "options": model.options,
        }),
    );
    // The declared options (the legacy `Task::create_message` options
    // set): a package-less step announced the options its declaration
    // carried, which is what a channel filter's `tag` matches. A `uses`
    // step's act payload carries the resolved options already.
    let options = find_step_in_model(&model, node)
        .map(|step| step.options.clone())
        .unwrap_or_default();
    if !options.is_empty() {
        inputs.insert(
            crate::utils::consts::ACT_OPTIONS_KEY.to_string(),
            serde_json::to_value(options).unwrap_or(serde_json::Value::Null),
        );
    }
    let mut payload = serde_json::json!({
        "tid": task_id.wire(&snapshot.process_id),
        "nid": node,
        "name": facts.name,
        "kind": facts.message_kind.to_string(),
        "inputs": serde_json::Value::Object(inputs),
    });
    if let Some((code, message)) = error {
        payload["error"] = serde_json::json!({ "code": code, "message": message });
    } else if let Some(outputs) = outputs {
        payload["outputs"] = message_outputs(outputs, task_id);
    }
    Ok(Some(payload))
}

/// The message payload of one operation's record: the act payload for a `uses`
/// step, the task payload otherwise. Both are the same legacy emission
/// (`Task::create_message`); only the fact set differs.
pub(crate) fn operation_message_payload(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
    node: &str,
    facts: Option<&ActFacts>,
    outputs: Option<&[VariableWrite]>,
    error: Option<(&str, &str)>,
) -> Result<Option<serde_json::Value>> {
    if let Some(payload) = act_message_payload(snapshot, task_id, node, facts, outputs, error)? {
        return Ok(Some(payload));
    }
    task_message_payload(snapshot, task_id, node, outputs, error)
}

/// The joined container's own completion message: a package-less
/// container speaks here (its children settled and it reached its terminal
/// state — the record is sequence-qualified, so a container that also
/// announced an act keeps both). A `uses` step's act announced itself at
/// the fan-out, so the join adds nothing for it: the legacy step's own
/// messages are suppressed once it carries an act.
pub(crate) fn attach_join_message(
    snapshot: &ProcessSnapshot,
    plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
    parent: &TaskId,
    operation: &OperationIntent,
) -> Result<()> {
    let Some(task) = snapshot.tasks.get(parent) else {
        return Ok(());
    };
    if step_uses(snapshot, parent, &task.model_node)?.is_some() {
        return Ok(());
    }
    if let Some(payload) = task_message_payload(snapshot, parent, parent.node.as_str(), None, None)?
    {
        plan.outbox.push(OutboxIntent {
            key: format!(
                "completed:{}:{}:{}:{}",
                snapshot.process_id,
                parent.node.as_str(),
                parent.generation,
                operation.id.sequence
            ),
            topic: "task.completed".into(),
            payload: payload.to_string(),
        });
    }
    Ok(())
}
