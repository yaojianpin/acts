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
    Message, MessageState, Vars,
    scheduler::{
        core::outbox::records::{OutboxReceipt, OutboxRecord},
        shell::caps::delivery::{DeliveryOutcome, OutboxTransport},
    },
    utils::consts,
};
use std::sync::Arc;

use publisher::PublishError;

/// Map one delivered record onto its engine message, or `None` when the
/// record carries no payload (lifecycle notes such as `task.started` on an operation
/// that announced nothing).
///
/// Two payload shapes arrive here, both faithful to the legacy
/// `Task::create_message`: an *act* payload (a `uses` step's message, with its
/// package and resolved params) and a *task* payload (a package-less step, a
/// branch or the workflow root, which the legacy engine also announced — its
/// `on_task` emits every state change that is not `pending`/`running`). The
/// `type` is the node level (`workflow|branch|step`; the import's virtual
/// wrappers are graph-only and announce nothing); both shapes carry it
/// explicitly since C6. A record an earlier kernel wrote omits it on the act
/// shape, where `act` remains the default — the legacy act was the only shape
/// without `kind`.
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
        // An assignment notification is an event of its own: the act is still
        // waiting, so it carries the `created` state of a live message.
        "assignment.transferred" | "assignment.delegated" | "assignment.undelegated" => {
            MessageState::Created
        }
        // A remind is its own event too: the record addresses the act's
        // current owner and the act stays waiting.
        "remind.created" => MessageState::Created,
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
        // The act payload shape writes `step` — the model's own word for the
        // step level (see `doc/design/message.md`). A record without one is
        // a task-level message; `step` stays the guard.
        Some(kind) => kind.to_string(),
        None => "step".to_string(),
    };
    let mid = payload
        .get("mid")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let text = |name: &str| {
        payload
            .get(name)
            .and_then(|value| value.as_str())
            .map(String::from)
    };
    let rev = payload
        .get("rev")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    // The message's inputs: the scope inputs the operation recorded, plus the
    // resolved params/options the message carries (the legacy
    // `Task::create_message`).
    let mut inputs = Vars::new();
    if let Some(bag) = payload.get("inputs").and_then(|value| value.as_object()) {
        for (key, value) in bag {
            inputs.set(key, value.clone());
        }
    }
    // The assignment identities ride the inputs under `assign` as well as
    // the message's own `owner`/`assignee`/`delegator` fields: `owner` is the
    // act's owner, `assignee` is whoever executes it now (the owner, or the
    // delegatee while a delegation is active), and `delegator` names the
    // owner who granted that delegation (absent when none is active).
    let mut assign = serde_json::Map::new();
    for name in ["owner", "assignee", "delegator"] {
        if let Some(value) = text(name) {
            assign.insert(name.to_string(), serde_json::Value::String(value));
        }
    }
    if !assign.is_empty() {
        inputs.set("assign", serde_json::Value::Object(assign));
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

    // The id the planning commit pinned, when the payload carries one: the
    // message is then addressable before delivery, which is what an
    // assignment notification's `ref_msg_id` points at. A record from an
    // older kernel has none and keeps the delivery-time id.
    let id = payload
        .get("msg_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            // A record without a pinned id still needs one identity across
            // retries. The record's own durable identity (pid + key) is the
            // stable input; hashing it means a duplicate dispatch converges on
            // the same canonical message instead of minting another.
            crate::utils::stable_id(
                "msg_",
                &[&record.id.process_id, &record.id.key, &record.topic],
            )
        });

    // The record's event time, pinned by the commit that wrote the payload
    // (`ts`); a payload an earlier shape wrote without one stamps at
    // mapping time.
    let timestamp = payload
        .get("ts")
        .and_then(|value| value.as_i64())
        .unwrap_or_else(crate::utils::time::time_millis);

    Some(Message {
        id,
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
        timestamp,
        owner: text("owner"),
        assignee: text("assignee"),
        delegator: text("delegator"),
        rev,
        ref_msg_id: text("ref_msg_id"),
    })
}

/// Drain a delivery pass into engine messages: the driver's transport runs
/// the records through [`message_from_record`], hands them to the durable
/// publisher, and only then notifies the online subscribers.
///
/// The durable publisher owns the message window now (it reads the projected
/// task row). The serving runtime constructs the forwarder with
/// [`MessageForwarder::durable`]; the test-only [`MessageForwarder::new`]
/// keeps the old in-memory window so unit tests can observe mapping and
/// notification without an engine.
pub(crate) struct MessageForwarder<F> {
    emit: F,
    publisher: Option<Arc<DurableMessagePublisher>>,
    /// The instant each `(pid, tid)` was first announced as `created` — test
    /// fallback only; production timing comes from the durable task row.
    #[cfg(test)]
    started: parking_lot::Mutex<std::collections::HashMap<(String, String), i64>>,
}

impl<F> MessageForwarder<F>
where
    F: Fn(&Message),
{
    /// Test-only mapping/notification transport. It is not a production
    /// delivery: its receipt is the legacy string and no durable message or
    /// delivery row is created.
    #[cfg(test)]
    pub(crate) fn new(emit: F) -> Self {
        Self {
            emit,
            publisher: None,
            started: parking_lot::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The serving transport: persist the canonical message and every matching
    /// ack channel's delivery before the outbox record is confirmed, then
    /// notify online subscribers.
    pub(crate) fn durable(emit: F, publisher: Arc<DurableMessagePublisher>) -> Self {
        Self {
            emit,
            publisher: Some(publisher),
            #[cfg(test)]
            started: parking_lot::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl<F> crate::scheduler::shell::caps::Capability for MessageForwarder<F>
where
    F: Fn(&Message) + Send + Sync,
{
    fn key(&self) -> &str {
        "kernel.delivery"
    }
}

#[async_trait::async_trait]
impl<F> OutboxTransport for MessageForwarder<F>
where
    F: Fn(&Message) + Send + Sync,
{
    async fn deliver(&self, record: &OutboxRecord) -> DeliveryOutcome {
        let Some(mut message) = message_from_record(record) else {
            return DeliveryOutcome::Delivered {
                receipt: OutboxReceipt::legacy("lifecycle"),
            };
        };
        if let Some(publisher) = &self.publisher {
            return match publisher.publish(&mut message).await {
                Ok(receipt) => {
                    // Online notification is now only a latency optimization:
                    // the durable message and delivery rows already exist, so
                    // a lost notification is recovered by the retry timer.
                    (self.emit)(&message);
                    DeliveryOutcome::Delivered { receipt }
                }
                Err(PublishError::Corrupt(reason)) => DeliveryOutcome::Quarantine { reason },
                Err(PublishError::Store(error)) => DeliveryOutcome::RetryAt {
                    available_at_ms: crate::utils::time::time_millis()
                        + DELIVERY_STORE_RETRY_BACKOFF_MS,
                    error: error.to_string(),
                },
            };
        }
        #[cfg(test)]
        {
            let now = crate::utils::time::time_millis();
            let key = (message.pid.clone(), message.tid.clone());
            if message.state == MessageState::Created {
                message.start_time = now;
                self.started.lock().insert(key, now);
            } else if message.state.is_completed() {
                // Test fallback: a terminal record closes the window opened by
                // the first `created` this forwarder saw. Production uses the
                // durable task row instead (see `DurableMessagePublisher`).
                message.start_time = self.started.lock().remove(&key).unwrap_or(now);
                message.end_time = now.max(message.start_time);
            }
            (self.emit)(&message);
            return DeliveryOutcome::Delivered {
                receipt: OutboxReceipt::legacy("message"),
            };
        }
        // Unreachable: a serving build has no constructor without a durable
        // publisher. Fail closed rather than confirming what was not stored.
        #[cfg(not(test))]
        DeliveryOutcome::Quarantine {
            reason: "message forwarder has no durable publisher".into(),
        }
    }
}

/// Backoff before a store failure is retried. The failure is local (no
/// external effect exists), so retrying quickly is correct; the lease still
/// bounds a hot loop if the store stays down.
const DELIVERY_STORE_RETRY_BACKOFF_MS: i64 = 1_000;

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
            "kind": "step",
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
        assert_eq!(message.r#type, "step");
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
                crate::scheduler::shell::facts::message_topic(state),
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
            "kind": "step",
            "uses": "acts.core.irq",
            "inputs": {},
            "mid": "wf1",
        })
        .to_string();
        let message = message_from_record(&record("message.created", &payload)).unwrap();
        assert_eq!(message.tid, "abc123");
        assert_eq!(message.nid, "s2");
        assert_eq!(message.r#type, "step");
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
    #[tokio::test]
    async fn a_forwarded_message_window_spans_created_to_terminal() {
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
            let forwarder =
                MessageForwarder::new(move |message: &Message| sink.lock().push(message.clone()));
            forwarder
                .deliver(&record("message.created", &payload))
                .await;
            forwarder.deliver(&record("task.completed", &payload)).await;
        }
        let seen = seen.lock();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].start_time > 0, "created opens the window");
        assert_eq!(seen[0].end_time, 0, "the window is still open");
        assert_eq!(seen[1].start_time, seen[0].start_time);
        assert!(seen[1].end_time >= seen[1].start_time);
        assert_eq!(seen[1].cost(), seen[1].end_time - seen[1].start_time);
    }

    /// An error record may carry only one of the two failure facts; each one
    /// lands in both the inputs and the outputs on its own.
    #[test]
    fn an_error_record_may_carry_only_one_failure_fact() {
        let only_code = message_from_record(&record(
            "task.failed",
            &serde_json::json!({
                "nid": "s1",
                "name": "s1",
                "uses": "test.fail",
                "inputs": {"params": {}},
                "mid": "wf1",
                "error": {"code": "EPACKAGE"},
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(
            only_code
                .inputs
                .get::<String>(consts::ACT_ERR_CODE)
                .as_deref(),
            Some("EPACKAGE")
        );
        assert_eq!(
            only_code
                .outputs
                .get::<String>(consts::ACT_ERR_CODE)
                .as_deref(),
            Some("EPACKAGE")
        );
        assert!(
            only_code
                .outputs
                .get::<String>(consts::ACT_ERR_MESSAGE)
                .is_none()
        );

        let only_message = message_from_record(&record(
            "task.failed",
            &serde_json::json!({
                "nid": "s1",
                "name": "s1",
                "uses": "test.fail",
                "inputs": {"params": {}},
                "mid": "wf1",
                "error": {"message": "boom"},
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(
            only_message
                .outputs
                .get::<String>(consts::ACT_ERR_MESSAGE)
                .as_deref(),
            Some("boom")
        );
        assert!(
            only_message
                .outputs
                .get::<String>(consts::ACT_ERR_CODE)
                .is_none()
        );
    }

    /// A payload whose `inputs` is not an object is still a message: the
    /// payload shape is only the payload, never the topic's identity.
    #[test]
    fn a_payload_without_an_inputs_bag_still_maps() {
        let message = message_from_record(&record(
            "message.created",
            &serde_json::json!({
                "nid": "s1",
                "name": "s1",
                "kind": "step",
                "inputs": "not-a-bag",
                "mid": "wf1",
            })
            .to_string(),
        ))
        .unwrap();
        assert!(message.inputs.is_empty());
    }

    /// A terminal record the forwarder never saw `created` for closes a
    /// zero-length window at the delivery instant instead of leaving the
    /// message with no end (a restarted forwarder, or a skip before any
    /// announcement).
    #[tokio::test]
    async fn an_unannounced_terminal_message_closes_at_the_delivery_instant() {
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
            let forwarder =
                MessageForwarder::new(move |message: &Message| sink.lock().push(message.clone()));
            forwarder.deliver(&record("task.skipped", &payload)).await;
        }
        let seen = seen.lock();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].start_time > 0);
        assert_eq!(seen[0].end_time, seen[0].start_time);
        assert_eq!(seen[0].cost(), 0);
    }

    /// A record with no payload maps onto no message but still confirms the
    /// delivery: the lifecycle note is a success, not an error.
    #[tokio::test]
    async fn a_lifecycle_note_confirms_delivery_without_a_message() {
        use crate::scheduler::shell::caps::delivery::DeliveryOutcome;
        let forwarder = MessageForwarder::new(|_: &Message| {
            panic!("a lifecycle note must not reach the engine channel")
        });
        let outcome = forwarder.deliver(&record("task.started", "")).await;
        match outcome {
            DeliveryOutcome::Delivered { receipt } => {
                assert_eq!(receipt.as_legacy(), Some("lifecycle"))
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    /// A record with a payload is delivered as a message, not as a lifecycle
    /// note: the receipt names the other outcome.
    #[tokio::test]
    async fn a_payload_record_confirms_delivery_as_a_message() {
        use crate::scheduler::shell::caps::delivery::DeliveryOutcome;
        let payload = serde_json::json!({ "nid": "s1" }).to_string();
        let forwarder = MessageForwarder::new(|_: &Message| {});
        let outcome = forwarder
            .deliver(&record("message.created", &payload))
            .await;
        match outcome {
            DeliveryOutcome::Delivered { receipt } => {
                assert_eq!(receipt.as_legacy(), Some("message"))
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    /// A payload without `nid` is not a task/act message (it names no model
    /// node); it maps onto `None` rather than a message keyed by nothing.
    #[test]
    fn a_payload_without_a_node_id_is_not_a_message() {
        assert!(message_from_record(&record("message.created", r#"{"name":"x"}"#)).is_none());
        assert!(message_from_record(&record("message.created", "not json")).is_none());
    }
}

mod payload;
mod publisher;

pub(crate) use payload::{attach_join_message, operation_message_payload, step_uses};
pub(crate) use publisher::DurableMessagePublisher;
