//! NATS server plugin for the acts workflow engine.
//!
//! Exposes the same message surface as `acts-plugin-grpc` over NATS core
//! with request/reply semantics:
//!
//! - **actions**: a remote publishes a JSON `{name, seq, data}` message to
//!   the actions subject (default `acts.cmd`); the plugin applies the action
//!   through the shared [`acts::actions`] dispatch table — the same one
//!   the gRPC plugin uses — and publishes the result back to the request's
//!   reply subject as `{name, ack, data, err}`. Snapshot updates work the
//!   same way (`snap:upsert` / `snap:remove`).
//! - **events**: each configured [`NatsChannelConfig`] registers an engine
//!   channel (its `type`/`state`/`uses`/`options` filters mirror the gRPC
//!   `MessageOptions`) and forwards every matched message to its subject as
//!   `{name, seq, data}` where `data` is the message payload and `seq` its
//!   id. Ack semantics equal the gRPC flow: the engine stores deliveries for
//!   the channel and re-sends unacked ones on its retry timer; a remote that
//!   handled a message acks it with the `msg:ack` action (`{id: seq}`) so
//!   redelivery stops.
//!
//! ```toml
//! [nats]
//! url = "nats://127.0.0.1:4222"
//! subject = "acts"
//! # raw payload cap, checked before the payload is parsed (1 KiB..64 MiB)
//! # max_payload_bytes = 1048576
//! # the payload token must not cross a plaintext wire
//! # tls_required = false
//! # refuse to start while a JetStream stream captures `<subject>.cmd`
//! # reject_jetstream_capture = false
//!
//! [[nats.channels]]
//! id = "ops"
//! type = "*"
//! state = "*"
//! uses = "*"
//! ```
//!
//! An inbound action passes a fixed admission order (ingress-security §4.3):
//! the raw payload is size-checked before `serde_json` sees any byte of it,
//! the envelope (`name`/`seq`/`ack`/`token`) is parsed under a shared parse
//! permit, the credential is resolved *before* the `data` field is decoded,
//! and only once the action's profile and the shared engine permits are held
//! does the decode and dispatch run — under the profile's deadline. Payload
//! content is never logged; sizes and codes are.
//!
//! Then register the plugin while building the engine:
//! `Engine::builder().add_plugin(&acts_plugin_nats::NatsPlugin::new())`.

use acts::{
    ActPlugin, CancellationToken, ChannelOptions, Engine, ParsePermit, Principal, TRANSPORT_NATS,
    Vars,
};
use async_nats::Client;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_json::value::RawValue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::Instrument;

pub use config::{DEFAULT_MAX_IN_FLIGHT, DEFAULT_MAX_PAYLOAD_BYTES, NatsChannelConfig, NatsConfig};
mod config;

/// The counters this transport feeds into the shared
/// [`acts::ingress::Metrics`] (reported by `sys:status`): one per inbound
/// action with its outcome (`ok`/`error`/`rejected`), one per refusal with
/// the bound that refused it, one per invalid credential.
const METRIC_REQUESTS_TOTAL: &str = "acts_ingress_requests_total";
const METRIC_REJECTED_TOTAL: &str = "acts_ingress_rejected_total";
const METRIC_AUTH_INVALID_TOTAL: &str = "acts_auth_invalid_total";

/// Wire format of an inbound action request — mirrors the gRPC `Message`
/// fields (`name`/`seq`/`ack`/`data`) plus the caller's `token`.
///
/// The token travels in the body rather than in a NATS header: the NATS
/// server authenticates a *connection*, and the engine has no access to the
/// broker's authenticated user, so the only place a per-request identity can
/// come from is the payload itself.
///
/// `data` is a borrowed [`RawValue`]: the envelope parse validates the JSON
/// and pins the bytes' extent, but the `data` text itself stays undecoded
/// (zero-copy, borrowed from the payload slice) until the caller is
/// authenticated and the action's permits are held — the decode is the
/// expensive step and the caller pays it only when admitted.
#[derive(Debug, Clone, Deserialize)]
struct WireMessage<'a> {
    name: String,
    #[serde(default)]
    seq: Option<String>,
    #[serde(default)]
    ack: Option<String>,
    #[serde(borrow, default)]
    data: Option<&'a RawValue>,
    #[serde(default)]
    token: Option<String>,
}

/// Wire format of an action reply (or event envelope).
///
/// An error reply carries the shared external-error contract alongside the
/// prose: `ecode` is the stable code the caller matches on (never the
/// prose), `correlation_id` matches the server-side log record with the
/// full detail, `retryable` says whether the same request may be retried.
#[derive(Debug, Clone, Serialize)]
struct WireReply {
    name: String,
    /// Echo of the request `seq`, for correlation.
    #[serde(skip_serializing_if = "Option::is_none")]
    ack: Option<String>,
    #[serde(default)]
    data: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    err: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ecode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
}

/// An error reply built from the shared external-error contract: the prose
/// in `err` (so older clients keep reading it), the contract fields beside
/// it.
fn reply_with_error(
    name: String,
    ack: Option<String>,
    error: &acts::export::ExternalError,
) -> WireReply {
    WireReply {
        name,
        ack,
        data: JsonValue::Null,
        err: Some(error.message.clone()),
        ecode: Some(error.code.clone()),
        correlation_id: Some(error.correlation_id.clone()),
        retryable: Some(error.retryable),
    }
}

/// A success reply: the action's own `data`, no error face.
fn reply_success(name: String, ack: Option<String>, data: JsonValue) -> WireReply {
    WireReply {
        name,
        ack,
        data,
        err: None,
        ecode: None,
        correlation_id: None,
        retryable: None,
    }
}

/// Publish one reply to a request's reply subject. A message without one is a
/// fire-and-forget publish: it still gets logs and metrics, just no reply.
async fn publish_reply(
    client: &Client,
    reply_subject: Option<async_nats::Subject>,
    reply: WireReply,
) {
    if let Some(subject) = reply_subject
        && let Err(err) = client.publish(subject, reply_bytes(&reply).into()).await
    {
        tracing::error!(error = %err, "nats action reply failed");
    }
}

async fn connect(config: &NatsConfig) -> std::result::Result<Client, String> {
    let opts = if let Some(token) = &config.token {
        async_nats::ConnectOptions::with_token(token.clone())
    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        async_nats::ConnectOptions::with_user_and_password(user.clone(), pass.clone())
    } else {
        async_nats::ConnectOptions::default()
    }
    // `[nats].tls_required`: the access token travels inside the action
    // payload, so over a plaintext link it is readable by anything between
    // the publisher and the broker — the broker must prove TLS before any
    // payload (and credential) goes out.
    .require_tls(config.tls_required);
    opts.connect(&config.url)
        .await
        .map_err(|e| format!("failed to connect to NATS({}): {e}", config.url))
}

/// Admission control for the actions arriving on the command subject: at most
/// `max_in_flight` of them execute at once, and one that cannot be admitted is
/// refused to its caller instead of being started.
///
/// Without it every message became a task of its own the moment it arrived,
/// with nothing bounding how many of them existed: a publisher that kept the
/// subject busy (a retry loop, a bug, or a hostile client) grew tasks — and the
/// deploys, process starts, store writes and outbound calls they run — for as
/// long as it kept publishing.
///
/// The overload is latched the way the store writer's saturation is: entering
/// and leaving the bound are one log line each, not one per refused message
/// (which would make the flood's cost a log line per message). The callers
/// themselves are told individually, by their replies.
///
/// This bound sits ON TOP of the shared engine permits
/// (`engine.ingress().try_enter_action`, the `[ingress]` budget): an admitted
/// action holds BOTH while it decodes and dispatches. The shared budget caps
/// what all transports spend together; this one caps what a single busy
/// subject spends even against an engine whose `[ingress]` is wide.
struct ActionsInFlight {
    max_in_flight: usize,
    permits: Arc<Semaphore>,
    saturated: Arc<AtomicBool>,
}

impl ActionsInFlight {
    fn new(max_in_flight: usize) -> Self {
        Self {
            max_in_flight,
            permits: Arc::new(Semaphore::new(max_in_flight)),
            saturated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Admit one action: `None` means the plugin is at its bound, and the
    /// caller must refuse the action rather than start it. The permit (in the
    /// returned guard) is held for the whole action, so the deploys, starts,
    /// store writes and outbound calls it runs all happen under it.
    fn try_enter(&self) -> Option<OwnedSemaphorePermit> {
        match self.permits.clone().try_acquire_owned() {
            Ok(permit) => {
                if self.saturated.swap(false, Ordering::AcqRel) {
                    tracing::info!(
                        max_in_flight = self.max_in_flight,
                        "nats actions dropped below their bound, accepting them again"
                    );
                }
                Some(permit)
            }
            Err(_) => {
                if !self.saturated.swap(true, Ordering::AcqRel) {
                    tracing::warn!(
                        max_in_flight = self.max_in_flight,
                        "nats actions reached their in-flight bound, refusing further ones"
                    );
                }
                None
            }
        }
    }

    /// The error a refused caller receives, naming the bound it hit.
    fn refusal(&self) -> String {
        format!(
            "too many actions in flight ({}); retry later",
            self.max_in_flight
        )
    }
}

/// One engine channel forwarded to its NATS subject.
fn register_channel(engine: &Engine, client: Client, channel: &NatsChannelConfig, base: &str) {
    let subject = channel
        .subject
        .clone()
        .unwrap_or_else(|| format!("{base}.evt.{}", channel.id.clone().unwrap_or_default()));
    let id = channel.id.clone().unwrap_or_else(|| subject.clone());
    let mut options = Vars::new();
    for (k, v) in &channel.options {
        options.set(k, v.clone());
    }
    let chan = engine.channel_with_options(&ChannelOptions {
        id: id.clone(),
        ack: true,
        r#type: channel.r#type.clone(),
        state: channel.state.clone(),
        uses: channel.uses.clone(),
        options,
    });

    let chan = Arc::new(chan);
    let subject_pub = subject.clone();
    chan.on_message(move |e| {
        let client = client.clone();
        let subject = subject_pub.clone();
        async move {
            let data = serde_json::to_value(e.inner()).unwrap_or_default();
            let reply = WireReply {
                name: e.name.clone(),
                ack: Some(e.id.clone()),
                data,
                err: None,
                ecode: None,
                correlation_id: None,
                retryable: None,
            };
            if let Err(err) = client
                .publish(subject.clone(), reply_bytes(&reply).into())
                .await
            {
                tracing::error!(subject = %subject, error = %err, "nats event publish failed");
            }
        }
    });
    tracing::info!(id = %id, subject = %subject, "nats channel registered");
}

/// Handle the inbound action subject: apply each command and reply to its
/// request subject (when present).
///
/// Every message passes the same admission order (ingress-security §4.3):
///
/// 1. the raw payload is size-checked against `[nats].max_payload_bytes`
///    *before* `serde_json` sees any byte of it;
/// 2. a shared parse permit is taken and the envelope (`name`/`seq`/`ack`/
///    `token`) is parsed under it — the permit covers the envelope parse
///    only;
/// 3. the NATS-local `max_in_flight` bound ([`ActionsInFlight`]) admits the
///    message;
/// 4. inside the spawned task: the credential is resolved (before any byte
///    of `data` is decoded), the action's profile re-checks the payload
///    size, the shared engine permits are taken, and only then is `data`
///    decoded;
/// 5. the dispatch runs under the profile deadline; an elapsed budget
///    answers `deadline_exceeded`.
///
/// A refused message is answered in its own envelope, so its caller learns
/// which of its actions was not started. One correlation id spans the
/// message: it is in the task's tracing span and in every reply. Payload
/// content is never logged — sizes and codes only.
async fn serve_actions(
    client: Client,
    subject: String,
    engine: Engine,
    shutdown: CancellationToken,
    payload_cap: usize,
    max_in_flight: usize,
) {
    let mut sub = match client.subscribe(subject.clone()).await {
        Ok(sub) => sub,
        Err(err) => {
            tracing::error!(subject = %subject, error = %err, "nats actions subscribe failed");
            return;
        }
    };
    tracing::info!(subject = %subject, max_in_flight, payload_cap, "nats actions subscription ready");

    let actions = ActionsInFlight::new(max_in_flight);
    loop {
        let msg = tokio::select! {
            _ = shutdown.cancelled() => break,
            msg = sub.next() => match msg {
                Some(msg) => msg,
                None => break,
            },
        };

        // One correlation id per inbound message: the tracing span and every
        // reply for this message quote the same id.
        let correlation_id = acts::export::new_correlation_id();
        let ingress = engine.ingress();
        let payload_bytes = msg.payload.len();

        // (1) transport cap — refused unread: the check reads the length
        // only, so no byte of the payload reaches serde yet.
        if payload_bytes > payload_cap {
            ingress.metrics.incr(
                METRIC_REQUESTS_TOTAL,
                &[
                    ("transport", TRANSPORT_NATS),
                    ("action", ""),
                    ("result", "rejected"),
                ],
            );
            ingress.metrics.incr(
                METRIC_REJECTED_TOTAL,
                &[("transport", TRANSPORT_NATS), ("reason", "payload_size")],
            );
            tracing::warn!(
                correlation_id,
                payload_bytes,
                cap = payload_cap,
                "nats payload exceeds the transport cap; refusing it unread"
            );
            let error = acts::export::ExternalError::new(
                acts::export::ExternalCode::Invalid,
                format!("payload of {payload_bytes} bytes exceeds the {payload_cap} byte limit"),
                correlation_id,
            );
            publish_reply(
                &client,
                msg.reply,
                reply_with_error(String::new(), None, &error),
            )
            .await;
            continue;
        }

        // (2) the envelope parse under the shared parse permit: parse
        // pressure refuses the message before parsing instead of queueing an
        // unbounded wait. The permit is dropped by [`parse_envelope`] — it
        // covers the envelope parse only.
        let Some(parse_permit) = ingress.try_enter_parse() else {
            ingress.metrics.incr(
                METRIC_REQUESTS_TOTAL,
                &[
                    ("transport", TRANSPORT_NATS),
                    ("action", ""),
                    ("result", "rejected"),
                ],
            );
            ingress.metrics.incr(
                METRIC_REJECTED_TOTAL,
                &[("transport", TRANSPORT_NATS), ("reason", "parse_pressure")],
            );
            tracing::warn!(
                correlation_id,
                payload_bytes,
                "nats parse pressure: refusing the message before parsing"
            );
            let error = acts::export::ExternalError::new(
                acts::export::ExternalCode::ResourceExhausted,
                "the engine is busy parsing payloads; try again later",
                correlation_id,
            );
            publish_reply(
                &client,
                msg.reply,
                reply_with_error(String::new(), None, &error),
            )
            .await;
            continue;
        };

        // The parse-failure reply carries the invalid_argument code; the
        // serde detail (bounded, control-char-free) names the fault for the
        // caller to fix. The envelope is parsed first: a caller that cannot
        // even be named (`name` missing) is told so before any admission.
        let cmd = match parse_envelope(&msg.payload, parse_permit) {
            Ok(cmd) => cmd,
            Err(err) => {
                ingress.metrics.incr(
                    METRIC_REQUESTS_TOTAL,
                    &[
                        ("transport", TRANSPORT_NATS),
                        ("action", ""),
                        ("result", "rejected"),
                    ],
                );
                tracing::error!(
                    correlation_id,
                    detail = %acts::export::safe_text(&err.to_string(), 1024),
                    payload_bytes,
                    "nats action payload parse failed"
                );
                let error = acts::export::ExternalError::new(
                    acts::export::ExternalCode::Invalid,
                    "invalid payload: the message is not a valid action envelope",
                    correlation_id,
                );
                publish_reply(
                    &client,
                    msg.reply,
                    reply_with_error(String::new(), None, &error),
                )
                .await;
                continue;
            }
        };

        // the name is user-controlled: bounded wherever it is logged, counted
        // or echoed back
        let action_label = acts::export::safe_text(&cmd.name, 256);

        // (3) the NATS-local bound: a message past it never becomes a task.
        let Some(local_permit) = actions.try_enter() else {
            ingress.metrics.incr(
                METRIC_REQUESTS_TOTAL,
                &[
                    ("transport", TRANSPORT_NATS),
                    ("action", action_label.as_str()),
                    ("result", "rejected"),
                ],
            );
            ingress.metrics.incr(
                METRIC_REJECTED_TOTAL,
                &[("transport", TRANSPORT_NATS), ("reason", "max_in_flight")],
            );
            tracing::warn!(name = %action_label, "nats actions are at their in-flight bound, refusing the message");
            let error = acts::export::ExternalError::new(
                acts::export::ExternalCode::ResourceExhausted,
                actions.refusal(),
                correlation_id,
            );
            publish_reply(
                &client,
                msg.reply,
                reply_with_error(action_label, cmd.seq.clone(), &error),
            )
            .await;
            continue;
        };

        // (4)/(5) run in the task — but only for an admitted message, so the
        // task per message is bounded work, not a flood amplifier. The task
        // holds the NATS-local permit from here; the shared engine permits
        // are its own admission step below. It carries an owned clone of the
        // payload (a `Bytes` clone shares the broker's buffer, it does not
        // copy it), so the borrowed `data` extent is re-pinned there and no
        // byte of the data is ever copied or decoded twice.
        let span = tracing::info_span!(
            "ingress",
            transport = acts::TRANSPORT_NATS,
            action = %action_label,
            cid = %correlation_id
        );
        let envelope = Envelope {
            payload: msg.payload.clone(),
            reply_subject: msg.reply.clone(),
        };
        let task_client = client.clone();
        let task_engine = engine.clone();
        tokio::spawn(
            async move {
                handle_action(
                    task_client,
                    task_engine,
                    envelope,
                    local_permit,
                    correlation_id,
                )
                .await;
            }
            .instrument(span),
        );
    }
}

/// The hand-off to one admitted message's task: the owned payload (a
/// `Bytes` clone shares the broker's buffer, it does not copy it) and the
/// reply subject. The envelope scalars are re-read from the re-parsed
/// envelope — no byte of the `data` is ever copied or decoded by this
/// hand-off.
struct Envelope {
    payload: bytes::Bytes,
    reply_subject: Option<async_nats::Subject>,
}

/// Admit and run one admitted action message: data re-extraction →
/// credential → profile size → shared engine permits → data decode →
/// dispatch under the profile deadline. Each step is its own function
/// ([`resolve_principal`], [`payload_size_ok`],
/// [`acts::Ingress::try_enter_action`], [`action_options`],
/// [`dispatch_in_budget`]) so the order is visible here and drivable in
/// tests without a broker.
///
/// The envelope was already parsed (and refusal answers already sent) in
/// [`serve_actions`]; this task re-scans its own owned payload to pin
/// `data`'s borrowed extent — a bounded scan of bytes that already passed
/// validation, no parse permit needed. The first byte of `data` is *decoded*
/// only at step (d), under both bounds.
async fn handle_action(
    client: Client,
    engine: Engine,
    envelope: Envelope,
    local_permit: OwnedSemaphorePermit,
    correlation_id: String,
) {
    // held for the whole task: the NATS-local bound stays latched while this
    // message decodes and dispatches
    let _local_permit = local_permit;
    let ingress = engine.ingress();
    let metrics = &ingress.metrics;
    let count_request = |action: &str, result: &str| {
        metrics.incr(
            METRIC_REQUESTS_TOTAL,
            &[
                ("transport", TRANSPORT_NATS),
                ("action", action),
                ("result", result),
            ],
        );
    };
    // a refusal reason for `acts_ingress_rejected_total`
    let count_rejected = |reason: &str| {
        metrics.incr(
            METRIC_REJECTED_TOTAL,
            &[("transport", TRANSPORT_NATS), ("reason", reason)],
        );
    };
    let reply_subject = envelope.reply_subject;

    // re-pin the data's extent: the envelope was validated in the loop, this
    // scan only finds the borrowed `data` slice inside the owned buffer
    let Ok(cmd) = serde_json::from_slice::<WireMessage>(&envelope.payload) else {
        metrics.incr(
            METRIC_REQUESTS_TOTAL,
            &[
                ("transport", TRANSPORT_NATS),
                ("action", ""),
                ("result", "rejected"),
            ],
        );
        tracing::error!(
            correlation_id,
            payload_bytes = envelope.payload.len(),
            "nats action payload re-parse failed (validated bytes changed shape)"
        );
        let error = acts::export::ExternalError::new(
            acts::export::ExternalCode::Invalid,
            "invalid payload: the message is not a valid action envelope",
            correlation_id.clone(),
        );
        publish_reply(
            &client,
            reply_subject,
            reply_with_error(String::new(), None, &error),
        )
        .await;
        return;
    };

    // the name is user-controlled: bounded wherever it is logged, counted or
    // echoed back
    let name = acts::export::safe_text(&cmd.name, 256);
    tracing::info!(
        name = %name,
        seq = cmd.seq.as_deref().map(|seq| acts::export::safe_text(seq, 128)).unwrap_or_default(),
        ack = cmd.ack.as_deref().map(|ack| acts::export::safe_text(ack, 128)).unwrap_or_default(),
        "nats action admitted"
    );

    // (a) credential first: an invalid token is refused before a byte of
    // `data` is decoded — the data never influences who the caller is, so it
    // cannot influence this answer either.
    let principal = match resolve_principal(&engine, &cmd.name, cmd.token.as_deref()).await {
        Ok(principal) => principal,
        Err(()) => {
            count_request(&name, "rejected");
            metrics.incr(METRIC_AUTH_INVALID_TOTAL, &[("transport", TRANSPORT_NATS)]);
            tracing::warn!(name = %name, "nats action unauthenticated");
            let error = acts::export::ExternalError::new(
                acts::export::ExternalCode::Unauthenticated,
                "the credential is invalid or expired",
                correlation_id.clone(),
            );
            publish_reply(
                &client,
                reply_subject,
                reply_with_error(name.clone(), cmd.seq.clone(), &error),
            )
            .await;
            return;
        }
    };

    // (b) the action's profile caps the raw payload — still before decode:
    // a caller past its profile does not get to spend the decode (or the
    // dispatch) on bytes the profile would refuse anyway.
    let profile = ingress.profile(&cmd.name);
    let payload_bytes = envelope.payload.len();
    if payload_size_ok(payload_bytes, profile.max_raw_bytes).is_err() {
        count_request(&name, "rejected");
        count_rejected("payload_size");
        tracing::warn!(
            name = %name,
            payload_bytes,
            cap = profile.max_raw_bytes,
            "nats payload exceeds the action profile"
        );
        let error = acts::export::ExternalError::new(
            acts::export::ExternalCode::Invalid,
            format!("payload exceeds the profile for {name}"),
            correlation_id.clone(),
        );
        publish_reply(
            &client,
            reply_subject,
            reply_with_error(name.clone(), cmd.seq.clone(), &error),
        )
        .await;
        return;
    }

    // (c) the shared engine permits (global + per-action): try-acquire, a
    // refusal is answered instead of queueing behind an unbounded wait.
    let _action_permits = match ingress.try_enter_action(&cmd.name) {
        Ok(permits) => permits,
        Err(reject) => {
            count_request(&name, "rejected");
            count_rejected(reject.reason());
            tracing::warn!(
                name = %name,
                reason = reject.reason(),
                "nats action refused by the shared ingress budget"
            );
            let error = acts::export::ExternalError::new(
                acts::export::ExternalCode::ResourceExhausted,
                format!(
                    "the engine is at capacity ({}); retry later",
                    reject.reason()
                ),
                correlation_id.clone(),
            );
            publish_reply(
                &client,
                reply_subject,
                reply_with_error(name.clone(), cmd.seq.clone(), &error),
            )
            .await;
            return;
        }
    };

    // (d) the first byte of `data` is decoded here — under the NATS-local
    // permit and the shared engine permits, inside the profile.
    let options = match action_options(cmd.data) {
        Ok(options) => options,
        Err(err) => {
            count_request(&name, "rejected");
            tracing::warn!(
                name = %name,
                error = %acts::export::safe_text(&err, 256),
                "nats action data rejected"
            );
            let error = acts::export::ExternalError::new(
                acts::export::ExternalCode::Invalid,
                err,
                correlation_id.clone(),
            );
            publish_reply(
                &client,
                reply_subject,
                reply_with_error(name.clone(), cmd.seq.clone(), &error),
            )
            .await;
            return;
        }
    };

    // (e) dispatch under the profile deadline
    let budget = Duration::from_millis(profile.deadline_ms);
    match dispatch_in_budget(
        budget,
        acts::actions::apply_as(&engine, &principal, &cmd.name, options),
    )
    .await
    {
        Ok(Ok(data)) => {
            count_request(&name, "ok");
            publish_reply(
                &client,
                reply_subject,
                reply_success(name.clone(), cmd.seq.clone(), data),
            )
            .await;
        }
        Ok(Err(err)) => {
            count_request(&name, "error");
            let error = acts::export::ExternalError::from_action(&err, &correlation_id);
            tracing::error!(name = %name, code = %error.code, "nats action failed");
            publish_reply(
                &client,
                reply_subject,
                reply_with_error(name.clone(), cmd.seq.clone(), &error),
            )
            .await;
        }
        Err(elapsed) => {
            count_request(&name, "error");
            tracing::warn!(
                name = %name,
                budget_ms = elapsed.budget_ms,
                "nats action exceeded its deadline; its effect may have applied"
            );
            let error = deadline_exceeded_error(&correlation_id, elapsed.budget_ms);
            publish_reply(
                &client,
                reply_subject,
                reply_with_error(name.clone(), cmd.seq.clone(), &error),
            )
            .await;
        }
    }
}

/// Parse the envelope while holding the parse permit: the permit is moved in
/// (the parse cannot run without one) and dropped here — it covers the
/// envelope parse only.
fn parse_envelope<'a>(
    payload: &'a [u8],
    _permit: ParsePermit,
) -> std::result::Result<WireMessage<'a>, serde_json::Error> {
    serde_json::from_slice(payload)
}

/// The principal one inbound action acts as: `Err(())` is an invalid
/// credential, refused with `unauthenticated`. A missing credential resolves
/// to the anonymous caller — the ACL then decides per action what anonymous
/// may do — and so does an invalid token on an action whose payload *is* the
/// credential: the request that repairs a dead session must stay reachable.
async fn resolve_principal(
    engine: &Engine,
    action: &str,
    token: Option<&str>,
) -> std::result::Result<Principal, ()> {
    match engine.acl().resolve(token).await {
        acts::AuthenticationResult::Authenticated(principal) => Ok(principal),
        acts::AuthenticationResult::MissingCredentials => Ok(engine.acl().anonymous()),
        acts::AuthenticationResult::InvalidCredentials if acts::carries_own_credential(action) => {
            Ok(engine.acl().anonymous())
        }
        acts::AuthenticationResult::InvalidCredentials => Err(()),
    }
}

/// The raw-size guard: `Ok` while a payload is within `cap` bytes. Called
/// once against the transport cap before the payload is parsed, and once
/// against the action profile after the caller is authenticated — both
/// before any `data` decode.
fn payload_size_ok(payload_bytes: usize, cap: usize) -> std::result::Result<(), ()> {
    if payload_bytes <= cap {
        Ok(())
    } else {
        Err(())
    }
}

/// The dispatch deadline's expiry marker: the budget it names goes into the
/// reply and the log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BudgetExceeded {
    budget_ms: u64,
}

/// Run one future under a budget: elapse cancels the future and returns the
/// marker, so the caller gets its `deadline_exceeded` answer instead of
/// waiting forever on a wedged action.
async fn dispatch_in_budget<F: Future>(
    budget: Duration,
    fut: F,
) -> std::result::Result<F::Output, BudgetExceeded> {
    tokio::time::timeout(budget, fut)
        .await
        .map_err(|_| BudgetExceeded {
            budget_ms: budget.as_millis() as u64,
        })
}

/// The error an elapsed budget answers: code `deadline_exceeded`, not
/// retryable — the effect may already have applied, so the caller
/// reconciles via reads rather than re-running the action blind.
fn deadline_exceeded_error(correlation_id: &str, budget_ms: u64) -> acts::export::ExternalError {
    acts::export::ExternalError {
        code: "deadline_exceeded".to_string(),
        message: format!(
            "the action did not finish within its {budget_ms} ms budget; its effect may have applied — reconcile via reads"
        ),
        correlation_id: correlation_id.to_string(),
        retryable: false,
    }
}

fn reply_bytes(reply: &WireReply) -> String {
    serde_json::to_string(reply).unwrap_or_else(|_| "{}".to_string())
}

/// Action options from an inbound request's `data` field.
///
/// When present, `data` MUST be a JSON object: a malformed payload is a
/// caller error, never an empty option set. Defaulting to empty options would
/// run the action's global branch (e.g. `msg:clear` clearing every error
/// delivery) instead of rejecting the request.
///
/// The input is the borrowed [`RawValue`] from the envelope: the JSON text is
/// decoded here — after authentication, under the action's permits and
/// inside its profile — because the decode is the expensive step and the
/// caller pays it only when admitted.
fn action_options(data: Option<&RawValue>) -> std::result::Result<Vars, String> {
    let Some(raw) = data else {
        return Ok(Vars::new());
    };
    match serde_json::from_str::<JsonValue>(raw.get()) {
        Ok(JsonValue::Object(map)) => Ok(Vars::from(map)),
        Ok(_) => Err("invalid payload: `data` must be a JSON object".to_string()),
        Err(err) => Err(format!(
            "invalid payload: `data` is not decodable JSON ({})",
            acts::export::safe_text(&err.to_string(), 256)
        )),
    }
}

/// The strict-startup JetStream check (`[nats].reject_jetstream_capture`):
/// list every stream on the broker (paginated, via the JetStream management
/// API) and fail on the first one whose subjects capture the command subject
/// — directly or through a wildcard. A stream holding `<prefix>.cmd`
/// persists and redelivers command messages, and with the access token
/// living in the payload a replayed command is a replayed credential.
///
/// A listing that cannot run (permissions, request failure) is itself a
/// failure: the check must be able to say "clean" for the start to pass.
async fn ensure_no_jetstream_capture(
    client: &Client,
    base: &str,
) -> std::result::Result<(), String> {
    let cmd_subject = format!("{base}.cmd");
    let jetstream = async_nats::jetstream::new(client.clone());
    let mut streams = jetstream.streams();
    while let Some(info) = streams.next().await {
        let info = info.map_err(|err| {
            format!(
                "the jetstream capture check could not run: listing the broker's streams failed ({})",
                acts::export::safe_text(&err.to_string(), 256)
            )
        })?;
        for subject in &info.config.subjects {
            if subjects_overlap(subject, &cmd_subject) {
                return Err(format!(
                    "jetstream stream '{}' captures the actions subject '{cmd_subject}' via its subject '{subject}'; its subjects must exclude it — a stream holding the command subject persists and replays command messages, access token included",
                    info.config.name
                ));
            }
        }
    }
    tracing::info!(subject = %cmd_subject, "no jetstream stream captures the actions subject");
    Ok(())
}

/// Whether two NATS subjects can share a message: token by token, where `*`
/// matches exactly one token and `>` (final position) matches one or more
/// trailing tokens. Symmetric — either side's wildcard covers the other.
fn subjects_overlap(a: &str, b: &str) -> bool {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        let (l, r) = (left.next(), right.next());
        match (l, r) {
            (Some(l), Some(r)) => {
                if l == ">" || r == ">" {
                    // both sides have at least the token in front of the `>`,
                    // so the rest is covered
                    return true;
                }
                if l != "*" && r != "*" && l != r {
                    return false;
                }
            }
            // one subject ended: leftover tokens on the other side have
            // nothing to land on (a `>` would have returned above)
            _ => return l.is_none() && r.is_none(),
        }
    }
}

#[derive(Clone)]
pub struct NatsPlugin {
    /// The connection established in `prepare`: a broker that refuses it fails
    /// the engine start instead of leaving a "running" engine with no
    /// transport (the old spawn-and-log behaviour).
    client: Arc<tokio::sync::Mutex<Option<Client>>>,
    /// The plugin's tasks, joined by `shutdown`.
    tasks: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl NatsPlugin {
    pub fn new() -> Self {
        Self {
            client: Arc::new(tokio::sync::Mutex::new(None)),
            tasks: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }
}

impl Default for NatsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ActPlugin for NatsPlugin {
    /// Connect to the broker. Registering the channels and serving only start
    /// after every plugin prepared, so a refused connection is a start
    /// failure with a reason.
    async fn prepare(&self, engine: &Engine) -> acts::Result<()> {
        let config: NatsConfig = engine.config().get("nats").unwrap_or_default();
        let client = connect(&config).await.map_err(acts::ActError::Config)?;
        // strict startup: no stream may capture the command subject (fail
        // closed — a listing that cannot run fails the start too)
        if config.reject_jetstream_capture {
            ensure_no_jetstream_capture(&client, &config.subject)
                .await
                .map_err(acts::ActError::Config)?;
        }
        let mut slot = self.client.lock().await;
        *slot = Some(client);
        Ok(())
    }

    async fn start(&self, engine: &Engine) -> acts::Result<()> {
        let config: NatsConfig = engine.config().get("nats").unwrap_or_default();
        let shutdown = engine.shutdown_token();
        let client = self.client.lock().await.clone().ok_or_else(|| {
            acts::ActError::Config("nats plugin started without a prepared connection".to_string())
        })?;

        let base = config.subject.clone();
        let cmd_subject = format!("{base}.cmd");
        let payload_cap = config.max_payload_bytes();
        let max_in_flight = config.max_in_flight();
        {
            let client = client.clone();
            let engine = engine.clone();
            let shutdown = shutdown.clone();
            let handle = tokio::spawn(async move {
                serve_actions(
                    client,
                    cmd_subject,
                    engine,
                    shutdown,
                    payload_cap,
                    max_in_flight,
                )
                .await;
            });
            self.tasks.lock().await.push(handle);
        }

        for channel in &config.channels {
            register_channel(engine, client.clone(), channel, &base);
        }

        // One task owns the connection's lifetime: it drops the client after
        // the shutdown token fires, which closes the subscriptions.
        let watcher = {
            let client = client.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                shutdown.cancelled().await;
                drop(client);
                tracing::info!("nats plugin stopped");
            })
        };
        self.tasks.lock().await.push(watcher);

        Ok(())
    }

    async fn shutdown(&self) -> acts::Result<()> {
        let handles: Vec<tokio::task::JoinHandle<()>> = {
            let mut tasks = self.tasks.lock().await;
            std::mem::take(&mut *tasks)
        };
        for handle in handles {
            // The engine cancels the token before shutting plugins down; the
            // bound keeps a wedged broker client from blocking close forever.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        self.client.lock().await.take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts_acl::AclUsers;
    use serde_json::json;

    /// The in-flight bound is admission, not advice: an action beyond it is
    /// refused (its caller answers `busy`), and a finished action frees its
    /// slot for the next one.
    #[test]
    fn actions_are_admitted_only_under_the_bound() {
        let actions = ActionsInFlight::new(2);

        let first = actions.try_enter().expect("the first action is admitted");
        let second = actions.try_enter().expect("the second action is admitted");
        assert!(
            actions.try_enter().is_none(),
            "an action past the bound must be refused, not started"
        );
        assert!(
            actions.refusal().contains("too many actions in flight (2)"),
            "the refusal names the bound: {}",
            actions.refusal()
        );

        drop(first);
        assert!(
            actions.try_enter().is_some(),
            "a finished action must free its slot"
        );
        drop(second);
        assert!(actions.try_enter().is_some());
    }

    #[test]
    fn plugin_constructors() {
        let _ = NatsPlugin::new();
    }

    /// An engine without the NATS plugin (so no broker is contacted) but with
    /// the given config table — the pipeline fns run against its ingress and
    /// acl exactly as `serve_actions` would drive them.
    async fn engine_with(table: toml::Table) -> Engine {
        let config = acts::Config {
            data: Default::default(),
            table,
        };
        acts::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .expect("the test engine must start")
    }

    /// The transport cap refuses oversize payloads *unread*: the guard only
    /// reads the length, so a payload of bytes no parser could accept is
    /// still refused by the cap alone — before serde is called at all.
    #[test]
    fn oversized_payload_is_refused_unread() {
        let cap = 1024;
        assert!(payload_size_ok(cap, cap).is_ok(), "at the cap is fine");

        let junk = vec![0u8; cap + 1];
        assert!(
            payload_size_ok(junk.len(), cap).is_err(),
            "one byte over the cap is refused before serde sees any of it"
        );
    }

    /// Refusals are counted by reason on the shared metrics, so one chart
    /// shows where requests are turned away.
    #[test]
    fn rejections_are_counted_by_reason() {
        let ingress = acts::Ingress::new(&acts::Config::default());
        for _ in 0..2 {
            ingress.metrics.incr(
                METRIC_REJECTED_TOTAL,
                &[("transport", TRANSPORT_NATS), ("reason", "payload_size")],
            );
        }
        let snapshot = ingress.metrics.snapshot();
        assert_eq!(
            snapshot["acts_ingress_rejected_total{reason=payload_size,transport=nats}"], 2,
            "both refusals land on the same counter key"
        );
    }

    /// Envelope-first: a message with huge junk `data` (valid JSON, not an
    /// object) and an invalid token answers unauthenticated — the credential
    /// is resolved before any byte of `data` is decoded, so the decode error
    /// ("must be a JSON object") can never mask the auth failure.
    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_token_is_answered_before_the_data_decode() {
        let engine = engine_with(toml::Table::new()).await;
        let junk = "x".repeat(2 * 1024 * 1024);
        let payload = serde_json::to_vec(&json!({
            "name": "proc:start",
            "seq": "s-1",
            "token": "not-a-real-token",
            "data": junk,
        }))
        .unwrap();

        let cmd = parse_envelope(
            &payload,
            engine
                .ingress()
                .try_enter_parse()
                .expect("an idle engine admits the parse"),
        )
        .expect("a valid envelope parses while its data stays text");
        // the data stayed borrowed, undecoded text from the payload slice
        let raw = cmd.data.expect("the envelope keeps the data").get();
        assert!(raw.starts_with("\"xxx") && raw.ends_with("xxx\""));

        // auth refuses before the decode step is ever reached
        let outcome = resolve_principal(&engine, &cmd.name, cmd.token.as_deref()).await;
        assert!(
            outcome.is_err(),
            "an unknown token on a bare engine must be refused"
        );
        // for contrast: the same data WOULD have failed the decode with the
        // object error — which is exactly why auth must come first
        let decode = action_options(cmd.data).expect_err("a string is not an object");
        assert!(decode.contains("must be a JSON object"));

        // the refusal the caller sees is the auth one, not the decode one
        let error = acts::export::ExternalError::new(
            acts::export::ExternalCode::Unauthenticated,
            "the credential is invalid or expired",
            acts::export::new_correlation_id(),
        );
        assert_eq!(error.code, "unauthenticated");
    }

    /// A missing credential stays on the anonymous caller (the ACL then
    /// refuses per action), and an invalid token on a credential-carrying
    /// action stays reachable too: the login action is how a caller gets a
    /// new credential after its old one died.
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_and_own_credentials_stay_reachable() {
        let engine = engine_with(toml::Table::new()).await;
        assert!(
            resolve_principal(&engine, "proc:start", None).await.is_ok(),
            "a missing credential resolves to the anonymous caller"
        );
        assert!(
            resolve_principal(&engine, "acl:login", Some("junk"))
                .await
                .is_ok(),
            "acl:login carries its own credential and stays reachable"
        );
    }

    /// The action's profile re-checks the raw payload size after auth, still
    /// before decode: a body past the profile never reaches the decode step.
    #[tokio::test(flavor = "multi_thread")]
    async fn payload_over_the_action_profile_is_refused_before_decode() {
        let table: toml::Table =
            toml::from_str("[ingress.profile.\"model:deploy\"]\nmax_raw_bytes = 4194304\n")
                .unwrap();
        let engine = engine_with(table).await;

        let profile = engine.ingress().profile("model:deploy");
        assert_eq!(
            profile.max_raw_bytes,
            4 * 1024 * 1024,
            "the override must reach the profile (4 MiB, not the 8 MiB default)"
        );
        assert!(payload_size_ok(4 * 1024 * 1024, profile.max_raw_bytes).is_ok());

        let oversized = vec![0u8; 5 * 1024 * 1024];
        assert!(
            payload_size_ok(oversized.len(), profile.max_raw_bytes).is_err(),
            "a 5 MiB body over the 4 MiB deploy profile is refused before decode"
        );
    }

    /// The shared engine permits refuse under the `[ingress]` caps: the
    /// try-acquire names the bound it hit instead of queueing, and released
    /// permits are re-usable.
    #[tokio::test(flavor = "multi_thread")]
    async fn engine_permits_refuse_under_ingress_caps() {
        let table: toml::Table = toml::from_str(
            "[ingress]\nglobal_inflight = 2\n\n[ingress.profile.\"model:deploy\"]\nmax_inflight = 1\n",
        )
        .unwrap();
        let engine = engine_with(table).await;
        let ingress = engine.ingress();

        let deploy = ingress
            .try_enter_action("model:deploy")
            .expect("the first deploy fits its own cap");
        let second_deploy = match ingress.try_enter_action("model:deploy") {
            Ok(_) => panic!("the second deploy must hit its own cap"),
            Err(reject) => reject,
        };
        assert_eq!(
            second_deploy,
            acts::AdmissionReject::ActionFull {
                action: "model:deploy".to_string()
            }
        );

        let first = ingress
            .try_enter_action("proc:start")
            .expect("a second distinct action fits the global cap");
        let second = match ingress.try_enter_action("proc:start") {
            Ok(_) => panic!("the engine is at its global cap, the third action must refuse"),
            Err(reject) => reject,
        };
        assert_eq!(second, acts::AdmissionReject::GlobalFull);

        drop(deploy);
        assert!(
            ingress.try_enter_action("model:deploy").is_ok(),
            "a finished action frees its slots"
        );
        drop(first);
    }

    /// The envelope parse keeps `data` as borrowed text (the decoded view is
    /// the payload's own bytes) and a broken envelope is a parse error before
    /// any admission step.
    #[test]
    fn envelope_parses_first_and_borrows_the_data() {
        let body = br#"{"name":"proc:start","seq":"s-1","ack":"a-1","token":"t","data":{"id":1}}"#;
        let permit = acts::Ingress::new(&acts::Config::default())
            .try_enter_parse()
            .unwrap();
        let cmd = parse_envelope(&body[..], permit).unwrap();
        assert_eq!(cmd.name, "proc:start");
        assert_eq!(cmd.seq.as_deref(), Some("s-1"));
        assert_eq!(cmd.ack.as_deref(), Some("a-1"));
        assert_eq!(cmd.token.as_deref(), Some("t"));
        assert_eq!(cmd.data.expect("data is kept").get(), r#"{"id":1}"#);

        let permit = acts::Ingress::new(&acts::Config::default())
            .try_enter_parse()
            .unwrap();
        assert!(
            parse_envelope(br#"{"data":1}"#, permit).is_err(),
            "a missing `name` is an envelope error"
        );
    }

    /// The dispatch runs under the profile deadline: a future that outlives
    /// the budget is answered `deadline_exceeded` (not retryable — the effect
    /// may already have applied, the caller reconciles via reads), and one
    /// that finishes passes its result through.
    #[tokio::test]
    async fn dispatch_budget_answers_deadline_exceeded() {
        let exceeded = dispatch_in_budget(
            Duration::from_millis(20),
            tokio::time::sleep(Duration::from_secs(60)),
        )
        .await
        .expect_err("a sleep past the budget must exceed it");
        assert_eq!(exceeded.budget_ms, 20);

        let error = deadline_exceeded_error("cid-1", exceeded.budget_ms);
        assert_eq!(error.code, "deadline_exceeded");
        assert!(
            !error.retryable,
            "the caller reconciles via reads, not retries"
        );
        assert!(error.message.contains("20 ms"));

        let text = serde_json::to_string(&reply_with_error("proc:start".to_string(), None, &error))
            .unwrap();
        assert!(text.contains("\"ecode\":\"deadline_exceeded\""));

        let value: u8 = dispatch_in_budget(Duration::from_millis(50), async { 7u8 })
            .await
            .expect("an instant future finishes inside the budget");
        assert_eq!(value, 7);
    }

    /// The strict startup check matches the command subject against stream
    /// subjects including the wildcard domains: `*` covers one token, `>`
    /// covers one or more trailing tokens.
    #[test]
    fn jetstream_subject_overlap_detection() {
        let cases: &[(&str, &str, bool)] = &[
            ("acts.cmd", "acts.cmd", true),
            ("acts.>", "acts.cmd", true),
            ("acts.*", "acts.cmd", true),
            (">", "acts.cmd", true),
            ("*.cmd", "acts.cmd", true),
            ("acts.>", "acts.cmd.extra", true),
            // `>` needs one more token in front of it
            ("acts.cmd.>", "acts.cmd", false),
            ("acts", "acts.cmd", false),
            ("other.cmd", "acts.cmd", false),
            ("acts.*.cmd", "acts.cmd", false),
            ("acts.cmd.x", "acts.cmd", false),
        ];
        for (stream_subject, cmd, expect) in cases {
            assert_eq!(
                subjects_overlap(stream_subject, cmd),
                *expect,
                "{stream_subject} vs {cmd}"
            );
            assert_eq!(
                subjects_overlap(cmd, stream_subject),
                *expect,
                "symmetry: {cmd} vs {stream_subject}"
            );
        }
    }

    #[test]
    fn wire_reply_serde() {
        let reply = WireReply {
            name: "proc:start".to_string(),
            ack: Some("seq-1".to_string()),
            data: json!("pid-1"),
            err: None,
            ecode: None,
            correlation_id: None,
            retryable: None,
        };
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.contains("\"name\":\"proc:start\""));
        assert!(text.contains("\"ack\":\"seq-1\""));
        assert!(!text.contains("\"err\""));
        assert!(
            !text.contains("ecode"),
            "a success reply carries no error face: {text}"
        );

        // an error reply carries the contract fields beside the prose: the
        // caller matches on the code, correlates via the id, and knows
        // whether retrying the same request can work
        let error = acts::export::ExternalError::new(
            acts::export::ExternalCode::ResourceExhausted,
            "too many actions in flight (2); retry later",
            "correlation-1",
        );
        let text = serde_json::to_string(&reply_with_error(
            "proc:start".to_string(),
            Some("seq-1".to_string()),
            &error,
        ))
        .unwrap();
        assert!(text.contains("\"err\":\"too many actions in flight (2); retry later\""));
        assert!(text.contains("\"ecode\":\"resource_exhausted\""));
        assert!(text.contains("\"correlation_id\":\"correlation-1\""));
        assert!(
            text.contains("\"retryable\":true"),
            "resource_exhausted is retryable: {text}"
        );
    }

    #[test]
    fn action_options_accepts_absent_or_object_data() {
        assert!(action_options(None).unwrap().is_empty());

        let empty: Box<RawValue> = serde_json::from_str("{}").unwrap();
        assert!(action_options(Some(&empty)).unwrap().is_empty());

        let payload: Box<RawValue> = serde_json::from_str(r#"{"id": "d-1"}"#).unwrap();
        let options = action_options(Some(&payload)).unwrap();
        assert_eq!(options.get::<String>("id").unwrap(), "d-1");
    }

    /// A non-object `data` must be rejected, never converted to empty options:
    /// empty options on `msg:clear` mean "clear every error delivery".
    #[test]
    fn action_options_rejects_non_object_data() {
        for data in ["[]", "\"msg:clear\"", "1", "null", "true"] {
            let raw: Box<RawValue> = serde_json::from_str(data).unwrap();
            let err = action_options(Some(&raw)).expect_err("non-object data must be rejected");
            assert!(
                err.contains("must be a JSON object"),
                "unexpected error for data={data}: {err}"
            );
        }
    }

    /// A broker that refuses the connection fails the engine start: the
    /// connection moved from the old spawned task (which logged "disabled" and
    /// returned Ok) to `prepare`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_broker_fails_the_engine_start() {
        // Bind and drop: the port stays closed for the plugin's connect.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let table: toml::Table =
            toml::from_str(&format!("[nats]\nurl = \"nats://127.0.0.1:{port}\"\n")).unwrap();
        let config = acts::Config {
            data: Default::default(),
            table,
        };
        let started = acts::Engine::builder()
            .set_config(&config)
            .disable_acl()
            .add_plugin(&NatsPlugin::new())
            .start()
            .await;
        assert!(
            started.is_err(),
            "a refused broker must fail the start, not disable the plugin silently"
        );
    }

    // ── broker-backed cases ──────────────────────────────────────────────
    //
    // These run against a reachable NATS server (`ACTS_NATS_URL`, default
    // `nats://127.0.0.1:4222`, JetStream enabled) and skip — return before
    // asserting anything — when none answers, so plain `cargo test` without
    // a broker still passes. CI starts one.

    /// A client toward the test broker, or `None` when none answers.
    async fn broker() -> Option<async_nats::Client> {
        let url =
            std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
        match tokio::time::timeout(std::time::Duration::from_secs(2), async_nats::connect(url))
            .await
        {
            Ok(Ok(client)) => Some(client),
            _ => None,
        }
    }

    /// A subject unique to this process and call: parallel test binaries and
    /// repeated runs never share one.
    fn unique_subject(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("acts-test-{tag}-{}-{nanos}", std::process::id())
    }

    /// Start an engine whose NATS plugin serves `subject`, with `extra` as
    /// additional `[nats]` keys.
    async fn broker_engine(subject: &str, extra: &str) -> Engine {
        let url =
            std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
        let table: toml::Table = toml::from_str(&format!(
            "[nats]\nurl = \"{url}\"\nsubject = \"{subject}\"\n{extra}"
        ))
        .unwrap();
        let cfg = acts::Config {
            data: Default::default(),
            table,
        };
        Engine::builder()
            .set_config(&cfg)
            .disable_acl()
            .add_plugin(&NatsPlugin::new())
            .start()
            .await
            .unwrap()
    }

    /// One request, retried while the plugin's subscription is still being
    /// established (a request to a subject with no subscriber fails fast).
    async fn request(
        client: &async_nats::Client,
        subject: String,
        payload: Vec<u8>,
    ) -> async_nats::Message {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match client
                .request(subject.clone(), payload.clone().into())
                .await
            {
                Ok(reply) => return reply,
                Err(err) if tokio::time::Instant::now() < deadline => {
                    let _ = err;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(err) => panic!("the plugin never subscribed: {err}"),
            }
        }
    }

    fn reply_json(reply: &async_nats::Message) -> serde_json::Value {
        serde_json::from_slice(&reply.payload).expect("the reply is the wire envelope json")
    }

    /// The full request/reply path: a command reaches the engine through the
    /// plugin's subscription and its answer carries the ack echo and data.
    #[tokio::test(flavor = "multi_thread")]
    async fn actions_round_trip_over_a_broker() {
        let Some(client) = broker().await else { return };
        let subject = unique_subject("roundtrip");
        let engine = broker_engine(&subject, "").await;
        engine.ingress().metrics.snapshot();

        let payload = serde_json::to_vec(&json!({
            "name": "snap:upsert",
            "seq": "wire-1",
            "data": {"name": "profile", "scope": "u1", "rev": 1, "data": {}}
        }))
        .unwrap();
        let reply = request(&client, format!("{subject}.cmd"), payload).await;
        let value = reply_json(&reply);
        assert_eq!(value["name"], "snap:upsert", "{value}");
        assert_eq!(value["ack"], "wire-1", "{value}");
        assert_eq!(value["data"], true, "{value}");
        assert!(value.get("err").is_none(), "{value}");
        engine.close().await;
    }

    /// A malformed envelope answers the stable invalid_argument contract
    /// instead of being dropped.
    #[tokio::test(flavor = "multi_thread")]
    async fn parse_failure_answers_the_contract() {
        let Some(client) = broker().await else { return };
        let subject = unique_subject("parse");
        let engine = broker_engine(&subject, "").await;
        let reply = request(&client, format!("{subject}.cmd"), b"{not json".to_vec()).await;
        let value = reply_json(&reply);
        assert_eq!(value["ecode"], "invalid_argument", "{value}");
        assert!(value["correlation_id"].is_string(), "{value}");
        assert!(
            value["err"]
                .as_str()
                .unwrap_or_default()
                .contains("invalid payload"),
            "{value}"
        );
        engine.close().await;
    }

    /// The transport cap refuses a payload past `[nats].max_payload_bytes`
    /// without parsing it, and counts the refusal by reason.
    #[tokio::test(flavor = "multi_thread")]
    async fn oversize_payload_is_refused_unread() {
        let Some(client) = broker().await else { return };
        let subject = unique_subject("oversize");
        let engine = broker_engine(&subject, "max_payload_bytes = 1024\n").await;
        let reply = request(&client, format!("{subject}.cmd"), vec![b'x'; 2048]).await;
        let value = reply_json(&reply);
        assert_eq!(value["ecode"], "invalid_argument", "{value}");
        assert!(
            value["err"]
                .as_str()
                .unwrap_or_default()
                .contains("exceeds"),
            "{value}"
        );
        let metrics = engine.ingress().metrics.snapshot();
        assert!(
            metrics
                .keys()
                .any(|k| k.contains("reason=payload_size") && k.contains("transport=nats")),
            "the refusal is counted: {metrics:?}"
        );
        engine.close().await;
    }

    /// A tokenless caller under a real ACL answers unauthenticated through
    /// the wire contract (the anonymous principal holds no grant).
    #[tokio::test(flavor = "multi_thread")]
    async fn acl_denial_answers_the_contract() {
        let Some(client) = broker().await else { return };
        let subject = unique_subject("acl");
        let url =
            std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
        let table: toml::Table = toml::from_str(&format!(
            "[nats]\nurl = \"{url}\"\nsubject = \"{subject}\"\n"
        ))
        .unwrap();
        let cfg = acts::Config {
            data: Default::default(),
            table,
        };
        let engine = Engine::builder()
            .set_config(&cfg)
            .with_user_acl()
            .add_plugin(&NatsPlugin::new())
            .start()
            .await
            .unwrap();
        let payload = serde_json::to_vec(&json!({"name": "proc:start", "seq": "n1"})).unwrap();
        let reply = request(&client, format!("{subject}.cmd"), payload).await;
        let value = reply_json(&reply);
        assert_eq!(value["ecode"], "unauthenticated", "{value}");
        engine.close().await;
    }

    /// A configured channel forwards the engine's workflow messages to its
    /// subject, carrying the ack id the remote uses to ack.
    #[tokio::test(flavor = "multi_thread")]
    async fn event_channel_forwards_to_its_subject() {
        let Some(client) = broker().await else { return };
        let subject = unique_subject("events");
        let evt = format!("{subject}.evt");
        let engine = broker_engine(
            &subject,
            &format!(
                "[[nats.channels]]\nid = \"c1\"\nsubject = \"{evt}\"\ntype = \"*\"\nstate = \"*\"\nuses = \"*\"\n"
            ),
        )
        .await;
        let mut sub = client.subscribe(evt.clone()).await.unwrap();
        client.flush().await.unwrap();

        let model = acts::Workflow::new()
            .with_id(&format!("m-{subject}"))
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses("acts.core.irq", Vars::new().with("key", "nats-event"))
            });
        let executor = engine.executor(&acts::Principal::unrestricted());
        executor.model().deploy(&model, None).await.unwrap();
        executor.proc().start(&model.id, Vars::new()).await.unwrap();

        let message = tokio::time::timeout(std::time::Duration::from_secs(10), sub.next())
            .await
            .expect("the channel forwards a workflow message")
            .expect("the subscription is live");
        let value: serde_json::Value = serde_json::from_slice(&message.payload).unwrap();
        assert!(value["name"].is_string(), "{value}");
        assert!(value["ack"].is_string(), "{value}");
        engine.close().await;
    }

    /// `reject_jetstream_capture` fails the start while a stream captures
    /// the command subject, naming the stream and subject.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_capturing_stream_fails_the_start() {
        let Some(client) = broker().await else { return };
        let subject = unique_subject("js");
        let jetstream = async_nats::jetstream::new(client.clone());
        let stream_name = format!("CAPTURE{}", std::process::id());
        jetstream
            .create_stream(async_nats::jetstream::stream::Config {
                name: stream_name.clone(),
                subjects: vec![format!("{subject}.cmd")],
                ..Default::default()
            })
            .await
            .unwrap();

        let url =
            std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
        let table: toml::Table = toml::from_str(&format!(
            "[nats]\nurl = \"{url}\"\nsubject = \"{subject}\"\nreject_jetstream_capture = true\n"
        ))
        .unwrap();
        let cfg = acts::Config {
            data: Default::default(),
            table,
        };
        let started = Engine::builder()
            .set_config(&cfg)
            .disable_acl()
            .add_plugin(&NatsPlugin::new())
            .start()
            .await;
        let err = match started {
            Ok(_) => panic!("a capturing stream must fail the start"),
            Err(err) => err,
        };
        let text = err.to_string();
        assert!(text.contains(&stream_name), "the stream is named: {text}");
        assert!(text.contains(&subject), "the subject is named: {text}");

        let _ = jetstream.delete_stream(&stream_name).await;
    }

    /// `tls_required` refuses the plaintext test broker instead of carrying
    /// the access token over it.
    #[tokio::test(flavor = "multi_thread")]
    async fn tls_required_refuses_a_plaintext_broker() {
        let Some(_client) = broker().await else {
            return;
        };
        let url =
            std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
        let table: toml::Table = toml::from_str(&format!(
            "[nats]\nurl = \"{url}\"\nsubject = \"{}\"\ntls_required = true\n",
            unique_subject("tls")
        ))
        .unwrap();
        let cfg = acts::Config {
            data: Default::default(),
            table,
        };
        let started = Engine::builder()
            .set_config(&cfg)
            .disable_acl()
            .add_plugin(&NatsPlugin::new())
            .start()
            .await;
        assert!(
            started.is_err(),
            "a plaintext broker must be refused when TLS is required"
        );
    }
}
