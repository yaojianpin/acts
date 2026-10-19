//! Mapping the host's outbox records onto the engine's `Message`.
//!
//! The outbox is the durable fact (`message.created`, `task.completed`,
//! `task.failed`, `task.skipped` carrying the act payload); the driver's
//! transport maps each record onto the `Message` the legacy engine's task
//! events produced and hands it to the engine channel.
//!
//! Identity is split exactly like the read model (C5): `tid` is the execution
//! instance's wire digest, `nid` is the model node it instantiates. `nid` is
//! the node lookup key — code that needs the model node (`$step`, model
//! lookups, rows) reads `nid`; `tid` addresses the instance.

use crate::{
    Message, MessageState, NodeKind, Vars,
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
/// `type` is the legacy node level (`workflow|branch|step|act`); both shapes
/// carry it explicitly since C6. A record an earlier kernel wrote omits it on
/// the act shape, where `act` remains the default — the legacy act was the
/// only shape without `kind`.
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
    // The instance's wire (`tid`) rides the payload since C5; records an
    // earlier kernel wrote predate it and keep the node id as the tid (the
    // V2 degeneration — same split the row model's `nid` always had).
    let tid = payload
        .get("tid")
        .and_then(|value| value.as_str())
        .unwrap_or(&nid)
        .to_string();
    let uses = payload.get("uses").and_then(|value| value.as_str());
    let name = payload
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or(&nid)
        .to_string();
    let kind = match payload.get("kind").and_then(|value| value.as_str()) {
        Some(kind) => kind.to_string(),
        // An earlier kernel's act payload omitted it; `act` is the level.
        None => NodeKind::Act.to_string(),
    };
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
        tid,
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
///
/// The outbox payload carries no wall-clock facts (the aggregate has no
/// clocks), so the forwarder stamps the message's window as it observes the
/// execution: a `created` record opens it, a terminal record closes it. The
/// first and the last message of one execution instance therefore span the
/// same interval a client sees on the run's lifecycle event, and
/// `Message::cost()` is a real duration on every terminal message.
pub(crate) struct MessageForwarder<F> {
    emit: F,
    /// The instant each `(pid, tid)` was first announced as `created`.
    started: std::collections::HashMap<(String, String), i64>,
}

impl<F> MessageForwarder<F>
where
    F: FnMut(&Message),
{
    pub(crate) fn new(emit: F) -> Self {
        Self {
            emit,
            started: std::collections::HashMap::new(),
        }
    }
}

impl<F> OutboxTransport for MessageForwarder<F>
where
    F: FnMut(&Message) + Send + Sync,
{
    fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome {
        match message_from_record(record) {
            Some(mut message) => {
                let now = crate::utils::time::time_millis();
                let key = (message.pid.clone(), message.tid.clone());
                if message.state == MessageState::Created {
                    message.start_time = now;
                    self.started.insert(key, now);
                } else if message.state.is_completed() {
                    // A terminal record closes the window: the first `created`
                    // this forwarder saw is the start. Without one (a message
                    // observed after a restart, or a task skipped before it was
                    // ever announced) the window is the delivery instant, which
                    // keeps `end_time` a real timestamp and `cost()` non-negative.
                    message.start_time = self.started.remove(&key).unwrap_or(now);
                    message.end_time = now.max(message.start_time);
                }
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
            "kind": "act",
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

    /// Every topic the outbox writes maps onto its message state — all nine,
    /// the only table that turns a decided state into a message.
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
            ("message.created", MessageState::Created),
            ("task.completed", MessageState::Completed),
            ("task.submitted", MessageState::Submitted),
            ("task.backed", MessageState::Backed),
            ("task.cancelled", MessageState::Cancelled),
            ("task.failed", MessageState::Error),
            ("task.aborted", MessageState::Aborted),
            ("task.skipped", MessageState::Skipped),
            ("task.removed", MessageState::Removed),
        ] {
            let message = message_from_record(&record(topic, &payload))
                .unwrap_or_else(|| panic!("{topic} must map onto an act message"));
            assert_eq!(message.state, state, "{topic}");
        }
    }

    /// Every terminal state names the topic it announces; a state that turns
    /// terminal without one would be lost (the topic is the only way a state
    /// becomes a message). The match is exhaustive, so adding a task state
    /// forces a decision here.
    #[test]
    fn every_terminal_state_names_the_topic_it_announces() {
        use crate::scheduler::core::model::task_graph::TaskState as S;

        let topic_of = |state: S| -> Option<&'static str> {
            match state {
                S::Completed => Some("task.completed"),
                S::Submitted => Some("task.submitted"),
                S::Backed => Some("task.backed"),
                S::Cancelled => Some("task.cancelled"),
                S::Failed => Some("task.failed"),
                S::Aborted => Some("task.aborted"),
                S::Skipped => Some("task.skipped"),
                S::Removed => Some("task.removed"),
                S::Created | S::Ready | S::Running | S::Waiting => None,
            }
        };
        for state in [
            S::Created,
            S::Ready,
            S::Running,
            S::Waiting,
            S::Completed,
            S::Submitted,
            S::Backed,
            S::Cancelled,
            S::Failed,
            S::Aborted,
            S::Skipped,
            S::Removed,
        ] {
            assert_eq!(
                super::super::facts::message_topic(state),
                topic_of(state),
                "{state:?}: the topic a terminal state announces"
            );
            assert_eq!(
                state.is_terminal(),
                topic_of(state).is_some(),
                "{state:?}: terminal iff it announces a topic"
            );
        }
    }

    /// `nid` names the model node (the lookup key), `tid` the execution
    /// instance (C5's wire): consumers must never derive one from the other.
    #[test]
    fn nid_names_the_node_and_tid_the_instance() {
        let payload = serde_json::json!({
            "tid": "abc123",
            "nid": "s2",
            "name": "approve",
            "kind": "act",
            "uses": "acts.core.irq",
            "inputs": {},
            "mid": "wf1",
        })
        .to_string();
        let message = message_from_record(&record("message.created", &payload)).unwrap();
        assert_eq!(message.tid, "abc123");
        assert_eq!(message.nid, "s2");
        assert_eq!(message.r#type, "act");
    }

    #[test]
    fn lifecycle_notes_and_empty_payloads_are_not_messages() {
        assert!(message_from_record(&record("task.started", "")).is_none());
        assert!(message_from_record(&record("task.propagated", "")).is_none());
        assert!(message_from_record(&record("task.completed", "")).is_none());
        assert!(message_from_record(&record("fanout.selected", "{}")).is_none());
    }

    /// The forwarder stamps the window the payload cannot carry: the first
    /// `created` record opens it and the terminal record closes it, so every
    /// terminal task/act message has a non-zero end and a real `cost()`.
    #[test]
    fn a_forwarded_message_window_spans_created_to_terminal() {
        let payload = serde_json::json!({
            "nid": "s1",
            "tid": "t1",
            "name": "s1",
            "kind": "step",
            "inputs": {},
            "mid": "wf1",
        })
        .to_string();
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        {
            let sink = seen.clone();
            let mut forwarder =
                MessageForwarder::new(move |message: &Message| sink.lock().push(message.clone()));
            forwarder.deliver(&record("message.created", &payload));
            forwarder.deliver(&record("task.completed", &payload));
        }
        let seen = seen.lock();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].start_time > 0, "created opens the window");
        assert_eq!(seen[0].end_time, 0, "the window is still open");
        assert_eq!(seen[1].start_time, seen[0].start_time);
        assert!(seen[1].end_time >= seen[1].start_time);
        assert_eq!(seen[1].cost(), seen[1].end_time - seen[1].start_time);
    }
}
