//! NATS server plugin for the acts workflow engine.
//!
//! Exposes the same message surface as `acts-plugin-grpc` over NATS core
//! with request/reply semantics:
//!
//! - **actions**: a remote publishes a JSON `{name, seq, data}` message to
//!   the actions subject (default `acts.cmd`); the plugin applies the action
//!   through the shared [`acts_plugin_common`] dispatch table — the same one
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

use acts::{ActPlugin, ChannelOptions, Engine, Vars};
use async_nats::Client;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::Arc;

pub use config::{NatsChannelConfig, NatsConfig};
mod config;

/// Wire format of an inbound action request — mirrors the gRPC `Message`
/// fields (`name`/`seq`/`ack`/`data`).
#[derive(Debug, Clone, Deserialize)]
struct WireMessage {
    name: String,
    #[serde(default)]
    seq: Option<String>,
    #[serde(default)]
    ack: Option<String>,
    #[serde(default)]
    data: Option<JsonValue>,
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
async fn serve_actions(client: Client, subject: String, engine: Engine) {
    let mut sub = match client.subscribe(subject.clone()).await {
        Ok(sub) => sub,
        Err(err) => {
            tracing::error!(subject = %subject, error = %err, "nats actions subscribe failed");
            return;
        }
    };
    tracing::info!(subject = %subject, "nats actions subscription ready");

    while let Some(msg) = sub.next().await {
        let client = client.clone();
        let engine = engine.clone();
        tokio::spawn(async move {
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
                    return;
                }
            };
            tracing::info!(
                "nats do-action name={} seq={:?} ack={:?}",
                cmd.name,
                cmd.seq,
                cmd.ack
            );
            let options = cmd.data.clone().map(Vars::from).unwrap_or_default();
            let result = acts_plugin_common::apply(&engine, &cmd.name, options).await;
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

#[async_trait::async_trait]
impl ActPlugin for NatsPlugin {
    fn on_init(&self, engine: &Engine) -> acts::Result<()> {
        let engine = engine.clone();
        let config: NatsConfig = engine.config().get("nats").unwrap_or_default();

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
            let actions_client = client.clone();
            let actions_engine = engine.clone();
            tokio::spawn(async move {
                serve_actions(actions_client, cmd_subject, actions_engine).await;
            });

            let channels = config.channels.clone();
            for channel in &channels {
                register_channel(&engine, client.clone(), channel, &base);
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
