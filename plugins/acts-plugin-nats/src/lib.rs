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
//!
//! [[nats.channels]]
//! id = "ops"
//! type = "*"
//! state = "*"
//! uses = "*"
//! ```
//!
//! Then register the plugin while building the engine:
//! `Engine::builder().add_plugin(&acts_plugin_nats::NatsPlugin::new())`.

use acts::{ActPlugin, CancellationToken, ChannelOptions, Engine, Vars};
use async_nats::Client;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub use config::{DEFAULT_MAX_IN_FLIGHT, NatsChannelConfig, NatsConfig};
mod config;

/// Wire format of an inbound action request — mirrors the gRPC `Message`
/// fields (`name`/`seq`/`ack`/`data`) plus the caller's `token`.
///
/// The token travels in the body rather than in a NATS header: the NATS
/// server authenticates a *connection*, and the engine has no access to the
/// broker's authenticated user, so the only place a per-request identity can
/// come from is the payload itself.
#[derive(Debug, Clone, Deserialize)]
struct WireMessage {
    name: String,
    #[serde(default)]
    seq: Option<String>,
    #[serde(default)]
    ack: Option<String>,
    #[serde(default)]
    data: Option<JsonValue>,
    #[serde(default)]
    token: Option<String>,
}

/// Wire format of an action reply (or event envelope).
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
}

#[derive(Clone)]
pub struct NatsPlugin;

impl NatsPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NatsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

async fn connect(config: &NatsConfig) -> std::result::Result<Client, String> {
    let opts = if let Some(token) = &config.token {
        async_nats::ConnectOptions::with_token(token.clone())
    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        async_nats::ConnectOptions::with_user_and_password(user.clone(), pass.clone())
    } else {
        async_nats::ConnectOptions::default()
    };
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
/// An action is admitted only while the plugin is under `max_in_flight`
/// running ones ([`ActionsInFlight`]); a message beyond it is answered
/// `too many actions in flight` and never reaches the engine.
async fn serve_actions(
    client: Client,
    subject: String,
    engine: Engine,
    shutdown: CancellationToken,
    max_in_flight: usize,
) {
    let mut sub = match client.subscribe(subject.clone()).await {
        Ok(sub) => sub,
        Err(err) => {
            tracing::error!(subject = %subject, error = %err, "nats actions subscribe failed");
            return;
        }
    };
    tracing::info!(subject = %subject, max_in_flight, "nats actions subscription ready");

    let actions = ActionsInFlight::new(max_in_flight);
    loop {
        let msg = tokio::select! {
            _ = shutdown.cancelled() => break,
            msg = sub.next() => match msg {
                Some(msg) => msg,
                None => break,
            },
        };

        // Parse before admitting: the refusal answers the same envelope a run
        // would, so a caller learns which of its actions was not started.
        let cmd: WireMessage = match serde_json::from_slice(&msg.payload) {
            Ok(cmd) => cmd,
            Err(err) => {
                tracing::error!(error = %err, "nats action payload parse failed");
                if let Some(subject) = msg.reply {
                    let out = WireReply {
                        name: String::new(),
                        ack: None,
                        data: JsonValue::Null,
                        err: Some(format!("invalid payload: {err}")),
                    };
                    let _ = client.publish(subject, reply_bytes(&out).into()).await;
                }
                continue;
            }
        };

        // At the bound: refuse instead of spawning. A task per message was
        // unbounded work — and each one ran a full action — so a busy subject
        // could exhaust the engine from outside.
        let Some(permit) = actions.try_enter() else {
            if let Some(subject) = msg.reply {
                let out = WireReply {
                    name: cmd.name,
                    ack: cmd.seq,
                    data: JsonValue::Null,
                    err: Some(actions.refusal()),
                };
                let _ = client.publish(subject, reply_bytes(&out).into()).await;
            }
            continue;
        };

        let client = client.clone();
        let engine = engine.clone();
        tokio::spawn(async move {
            // released when the action is done, whatever it did
            let _permit = permit;
            tracing::info!(
                "nats do-action name={} seq={:?} ack={:?}",
                cmd.name,
                cmd.seq,
                cmd.ack
            );
            let options = match action_options(cmd.data) {
                Ok(options) => options,
                Err(err) => {
                    tracing::error!(name = %cmd.name, error = %err, "nats action payload rejected");
                    if let Some(subject) = msg.reply {
                        let out = WireReply {
                            name: cmd.name,
                            ack: cmd.seq,
                            data: JsonValue::Null,
                            err: Some(err),
                        };
                        let _ = client.publish(subject, reply_bytes(&out).into()).await;
                    }
                    return;
                }
            };
            let principal = match engine.acl().authenticate(cmd.token.as_deref()) {
                Ok(principal) => principal,
                Err(err) => {
                    tracing::warn!(name = %cmd.name, error = %err, "nats action unauthenticated");
                    if let Some(subject) = msg.reply {
                        let out = WireReply {
                            name: cmd.name,
                            ack: cmd.seq,
                            data: JsonValue::Null,
                            err: Some(err.to_string()),
                        };
                        let _ = client.publish(subject, reply_bytes(&out).into()).await;
                    }
                    return;
                }
            };
            let result = acts::actions::apply_as(&engine, &principal, &cmd.name, options).await;
            let reply = match result {
                Ok(data) => WireReply {
                    name: cmd.name,
                    ack: cmd.seq,
                    data,
                    err: None,
                },
                Err(err) => WireReply {
                    name: cmd.name,
                    ack: cmd.seq,
                    data: JsonValue::Null,
                    err: Some(err.to_string()),
                },
            };
            if let Some(subject) = msg.reply
                && let Err(err) = client.publish(subject, reply_bytes(&reply).into()).await
            {
                tracing::error!(error = %err, "nats action reply failed");
            }
        });
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
fn action_options(data: Option<JsonValue>) -> std::result::Result<Vars, String> {
    match data {
        None => Ok(Vars::new()),
        Some(JsonValue::Object(map)) => Ok(Vars::from(map)),
        Some(_) => Err("invalid payload: `data` must be a JSON object".to_string()),
    }
}

#[async_trait::async_trait]
impl ActPlugin for NatsPlugin {
    fn on_init(&self, engine: &Engine) -> acts::Result<()> {
        let engine = engine.clone();
        let config: NatsConfig = engine.config().get("nats").unwrap_or_default();
        let shutdown = engine.shutdown_token();

        tokio::spawn(async move {
            let client = match connect(&config).await {
                Ok(client) => client,
                Err(err) => {
                    tracing::error!(error = %err, "nats plugin disabled");
                    return;
                }
            };

            let base = config.subject.clone();
            let cmd_subject = format!("{base}.cmd");
            let max_in_flight = config.max_in_flight();
            let actions_client = client.clone();
            let actions_engine = engine.clone();
            let actions_shutdown = shutdown.clone();
            tokio::spawn(async move {
                serve_actions(
                    actions_client,
                    cmd_subject,
                    actions_engine,
                    actions_shutdown,
                    max_in_flight,
                )
                .await;
            });

            let channels = config.channels.clone();
            for channel in &channels {
                register_channel(&engine, client.clone(), channel, &base);
            }

            shutdown.cancelled().await;
            tracing::info!("nats plugin stopped");
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn wire_reply_serde() {
        let reply = WireReply {
            name: "proc:start".to_string(),
            ack: Some("seq-1".to_string()),
            data: json!("pid-1"),
            err: None,
        };
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.contains("\"name\":\"proc:start\""));
        assert!(text.contains("\"ack\":\"seq-1\""));
        assert!(!text.contains("err"));
    }

    #[test]
    fn action_options_accepts_absent_or_object_data() {
        assert!(action_options(None).unwrap().is_empty());
        assert!(action_options(Some(json!({}))).unwrap().is_empty());

        let payload = action_options(Some(json!({ "id": "d-1" }))).unwrap();
        assert_eq!(payload.get::<String>("id").unwrap(), "d-1");
    }

    /// A non-object `data` must be rejected, never converted to empty options:
    /// empty options on `msg:clear` mean "clear every error delivery".
    #[test]
    fn action_options_rejects_non_object_data() {
        for data in [
            json!([]),
            json!("msg:clear"),
            json!(1),
            json!(null),
            json!(true),
        ] {
            let err = action_options(Some(data.clone()))
                .expect_err(&format!("data={data} must be rejected"));
            assert!(
                err.contains("must be a JSON object"),
                "unexpected error for data={data}: {err}"
            );
        }
    }
}
