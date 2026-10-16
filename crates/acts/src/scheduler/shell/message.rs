//! Mapping the host's outbox records onto the engine's `Message`.
//!
//! The outbox is the durable fact (`message.created`, `task.completed`,
//! `task.failed`, `task.skipped` carrying the act payload); the driver's
//! transport maps each record onto the `Message` the legacy engine's task
//! events produced and hands it to the engine channel. Identity matches the
//! shadow's action hook: the kernel keys executions by model node id, so
//! `tid`/`nid` are the node id.

use crate::{
    Message, MessageState, Vars,
    scheduler::core::outbox::{
        outbox_dispatch::{DeliveryOutcome, OutboxTransport},
        records::OutboxRecord,
    },
    utils::consts,
};

/// Map one delivered record onto its engine message, or `None` when the
/// record carries no payload (lifecycle notes such as `task.started` on an operation
/// that announced nothing).
///
/// Two payload shapes arrive here, both faithful to the legacy
/// `Task::create_message`: an *act* payload (a `uses` step's message, with its
/// package and resolved params) and a *task* payload (a package-less step, a
/// branch or the workflow root, which the legacy engine also announced — its
/// `on_task` emits every state change that is not `pending`/`running`). The
/// `type` is the legacy node kind; the act shape omits it.
pub(crate) fn message_from_record(record: &OutboxRecord) -> Option<Message> {
    let state = match record.topic.as_str() {
        "message.created" => MessageState::Created,
        "task.completed" => MessageState::Completed,
        "task.submitted" => MessageState::Submitted,
        "task.backed" => MessageState::Backed,
        "task.cancelled" => MessageState::Cancelled,
        "task.failed" => MessageState::Error,
        "task.aborted" => MessageState::Aborted,
        "task.skipped" => MessageState::Skipped,
        "task.removed" => MessageState::Removed,
        _ => return None,
    };
    let payload: serde_json::Value = serde_json::from_str(&record.payload).ok()?;
    let nid = payload.get("nid")?.as_str()?.to_string();
    let uses = payload.get("uses").and_then(|value| value.as_str());
    let name = payload
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or(&nid)
        .to_string();
    let kind = payload
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or("act")
        .to_string();
    let mid = payload
        .get("mid")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();

    // The message's inputs: the scope inputs the operation recorded, plus the
    // resolved params/options the message carries (the legacy
    // `Task::create_message`).
    let mut inputs = Vars::new();
    if let Some(bag) = payload.get("inputs").and_then(|value| value.as_object()) {
        for (key, value) in bag {
            inputs.set(key, value.clone());
        }
    }
    // The failure facts an error record adds: `ecode`/`message` are what a
    // consumer reads the failure from, so they land in the message's `outputs`
    // as well as its `inputs`.
    let mut outputs = Vars::new();
    if let Some(error) = payload.get("error").and_then(|value| value.as_object()) {
        if let Some(code) = error.get("code") {
            inputs.set(consts::ACT_ERR_CODE, code.clone());
            outputs.set(consts::ACT_ERR_CODE, code.clone());
        }
        if let Some(message) = error.get("message") {
            inputs.set(consts::ACT_ERR_MESSAGE, message.clone());
            outputs.set(consts::ACT_ERR_MESSAGE, message.clone());
        }
    }
    if let Some(out) = payload.get("outputs").and_then(|value| value.as_object()) {
        for (key, value) in out {
            outputs.set(key, value.clone());
        }
    }

    Some(Message {
        id: crate::utils::longid(),
        delivery_id: None,
        tid: nid.clone(),
        name,
        state,
        r#type: kind,
        pid: record.id.process_id.clone(),
        nid,
        mid,
        uses: uses.map(String::from),
        inputs,
        outputs,
        start_time: 0,
        end_time: 0,
        retry_times: 0,
        timestamp: crate::utils::time::time_millis(),
    })
}

/// Drain a delivery pass into engine messages: the driver's transport runs
/// the records through [`message_from_record`] and forwards the results.
pub(crate) struct MessageForwarder<F> {
    emit: F,
}

impl<F> MessageForwarder<F>
where
    F: FnMut(&Message),
{
    pub(crate) fn new(emit: F) -> Self {
        Self { emit }
    }
}

impl<F> OutboxTransport for MessageForwarder<F>
where
    F: FnMut(&Message) + Send + Sync,
{
    fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome {
        match message_from_record(record) {
            Some(message) => {
                (self.emit)(&message);
                DeliveryOutcome::Delivered {
                    receipt: "message".into(),
                }
            }
            None => DeliveryOutcome::Delivered {
                receipt: "lifecycle".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::outbox::records::OutboxId;
    use crate::scheduler::core::outbox::records::OutboxState;

    fn record(topic: &str, payload: &str) -> OutboxRecord {
        OutboxRecord {
            id: OutboxId {
                process_id: "p1".into(),
                key: format!("{topic}:k"),
            },
            topic: topic.into(),
            payload: payload.into(),
            state: OutboxState::Pending { available_at_ms: 0 },
            attempt: 0,
            last_fence: 0,
            revision: 1,
        }
    }

    #[test]
    fn a_created_record_maps_onto_the_act_message() {
        let payload = serde_json::json!({
            "nid": "s2",
            "name": "approve",
            "uses": "acts.core.irq",
            "inputs": {"params": {"key": "act1"}, "options": {"ui": true}},
            "mid": "wf1",
        })
        .to_string();
        let message = message_from_record(&record("message.created", &payload)).unwrap();
        assert_eq!(message.tid, "s2");
        assert_eq!(message.nid, "s2");
        assert_eq!(message.pid, "p1");
        assert_eq!(message.mid, "wf1");
        assert_eq!(message.r#type, "act");
        assert_eq!(message.state, MessageState::Created);
        assert_eq!(message.uses.as_deref(), Some("acts.core.irq"));
        assert!(message.is_irq());
        assert_eq!(
            message
                .inputs
                .get::<serde_json::Value>(consts::ACT_PARAMS_KEY)
                .unwrap()["key"],
            "act1"
        );
        // The act's options ride the message: a channel subscribed with an
        // option filter matches on them.
        assert_eq!(message.options().unwrap().get::<bool>("ui"), Some(true));
    }

    #[test]
    fn terminal_records_map_with_outputs_and_error_inputs() {
        let payload = serde_json::json!({
            "nid": "s1",
            "name": "s1",
            "uses": "test.func",
            "inputs": {"params": {}},
            "mid": "wf1",
            "outputs": {"n": 5},
        })
        .to_string();
        let completed = message_from_record(&record("task.completed", &payload)).unwrap();
        assert_eq!(completed.state, MessageState::Completed);
        assert_eq!(completed.outputs.get::<i32>("n"), Some(5));

        let failed_payload = serde_json::json!({
            "nid": "s1",
            "name": "s1",
            "uses": "test.fail",
            "inputs": {"params": {}},
            "mid": "wf1",
            "error": {"code": "EPACKAGE", "message": "boom"},
        })
        .to_string();
        let failed = message_from_record(&record("task.failed", &failed_payload)).unwrap();
        assert_eq!(failed.state, MessageState::Error);
        assert_eq!(
            failed.inputs.get::<String>(consts::ACT_ERR_CODE).as_deref(),
            Some("EPACKAGE")
        );

        let skipped = message_from_record(&record(
            "task.skipped",
            &serde_json::json!({"nid": "s", "name": "s", "uses": "test.irq", "inputs": {"params": {}}, "mid": "wf1"})
                .to_string(),
        ))
        .unwrap();
        assert_eq!(skipped.state, MessageState::Skipped);
    }

    /// Every decided state announces itself on its own topic — the act
    /// message the legacy engine emitted for that transition.
    #[test]
    fn decided_state_topics_map_onto_message_states() {
        let payload = serde_json::json!({
            "nid": "s1",
            "name": "s1",
            "uses": "test.irq",
            "inputs": {"params": {}},
            "mid": "wf1",
        })
        .to_string();
        for (topic, state) in [
            ("task.submitted", MessageState::Submitted),
            ("task.backed", MessageState::Backed),
            ("task.cancelled", MessageState::Cancelled),
            ("task.aborted", MessageState::Aborted),
            ("task.removed", MessageState::Removed),
        ] {
            let message = message_from_record(&record(topic, &payload))
                .unwrap_or_else(|| panic!("{topic} must map onto an act message"));
            assert_eq!(message.state, state, "{topic}");
        }
    }

    #[test]
    fn lifecycle_notes_and_empty_payloads_are_not_messages() {
        assert!(message_from_record(&record("task.started", "")).is_none());
        assert!(message_from_record(&record("task.propagated", "")).is_none());
        assert!(message_from_record(&record("task.completed", "")).is_none());
        assert!(message_from_record(&record("fanout.selected", "{}")).is_none());
    }
}
