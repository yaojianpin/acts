use serde::Deserialize;
use std::collections::HashMap;

/// How many inbound actions may execute at once when `[nats].max_in_flight`
/// does not say otherwise.
pub const DEFAULT_MAX_IN_FLIGHT: usize = 256;

/// Upper bound of `[nats].max_in_flight`: an action bound this high is already
/// no bound.
const MAX_IN_FLIGHT: usize = 65_536;

/// The raw payload cap when `[nats].max_payload_bytes` does not say otherwise:
/// 1 MiB. Deploy-sized actions are additionally capped by the broker's own
/// `max_payload` — this knob bounds what the plugin accepts, the broker what
/// it transports at all.
pub const DEFAULT_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// Floor of `[nats].max_payload_bytes`: below 1 KiB even the small actions
/// (`msg:ack`, `snap:remove`) cannot carry their arguments.
const MIN_MAX_PAYLOAD_BYTES: usize = 1024;

/// Ceiling of `[nats].max_payload_bytes`: past 64 MiB it is no cap at all.
const MAX_MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Plugin configuration, read from the engine config section `[nats]`.
#[derive(Debug, Clone, Deserialize)]
pub struct NatsConfig {
    /// NATS server url, e.g. `nats://127.0.0.1:4222`.
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// Subject prefix. Actions are received on `<prefix>.cmd`; each channel
    /// publishes its engine events to its own subject (default
    /// `<prefix>.evt.<channel-subject|id>`).
    #[serde(default = "default_subject")]
    pub subject: String,
    /// How many actions received on the actions subject may execute at once.
    ///
    /// Every message used to be answered by a task of its own as soon as it
    /// arrived, with nothing bounding how many of them existed: a publisher
    /// that kept the subject busy (a retry loop, a bug, a hostile client) grew
    /// tasks — and the deploys, process starts, store writes and outbound calls
    /// they run — for as long as it kept publishing. Past this bound an action
    /// is refused to its caller instead of started. `0` selects the default.
    #[serde(default)]
    pub max_in_flight: Option<usize>,
    /// The raw payload cap, in bytes, checked **before** the payload is
    /// parsed: an oversize message is refused unread, so no byte of it ever
    /// reaches `serde_json` (allocation, parse work, hostile-JSON probes are
    /// all out). Clamped to 1 KiB..64 MiB — a configured `0` is a floor, not
    /// an off switch.
    #[serde(default)]
    pub max_payload_bytes: Option<usize>,
    /// Require TLS on the broker connection (`require_tls`): the access token
    /// travels inside the action payload, and over a plaintext link it is
    /// readable by anything between the publisher and the broker — production
    /// must not cross plaintext.
    #[serde(default)]
    pub tls_required: bool,
    /// Refuse to start while a JetStream stream's subjects capture
    /// `<subject>.cmd`: a stream holding the command subject persists and
    /// redelivers command messages, and with the token living in the payload
    /// a replayed command is a replayed credential. When set, the check runs
    /// at startup (after the broker connect) and a stream overlap **or an
    /// unrunnable listing** fails the start — the check must be able to say
    /// "clean" for the start to pass.
    #[serde(default)]
    pub reject_jetstream_capture: bool,
    /// Engine message subscriptions forwarded to NATS. Empty by default —
    /// add one entry per remote subscriber, mirroring a gRPC `OnMessage`
    /// client whose `MessageOptions` are the filter fields below.
    #[serde(default)]
    pub channels: Vec<NatsChannelConfig>,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            url: default_url(),
            token: None,
            username: None,
            password: None,
            subject: default_subject(),
            max_in_flight: None,
            max_payload_bytes: None,
            tls_required: false,
            reject_jetstream_capture: false,
            channels: Vec::new(),
        }
    }
}

impl NatsConfig {
    /// How many inbound actions may run at once. Like the engine's own queue
    /// caps, an absent (or zero) field selects [`DEFAULT_MAX_IN_FLIGHT`], and
    /// the value is clamped so neither a zero-capacity nor an absurd bound is
    /// configurable.
    pub fn max_in_flight(&self) -> usize {
        match self.max_in_flight {
            Some(0) | None => DEFAULT_MAX_IN_FLIGHT,
            Some(max_in_flight) => max_in_flight.clamp(1, MAX_IN_FLIGHT),
        }
    }

    /// The raw payload cap, in bytes, refused before the payload is parsed.
    /// An absent field selects [`DEFAULT_MAX_PAYLOAD_BYTES`]; a configured
    /// value is clamped to 1 KiB..64 MiB, so a `0` is the floor — never an
    /// off switch.
    pub fn max_payload_bytes(&self) -> usize {
        match self.max_payload_bytes {
            None => DEFAULT_MAX_PAYLOAD_BYTES,
            Some(bytes) => bytes.clamp(MIN_MAX_PAYLOAD_BYTES, MAX_MAX_PAYLOAD_BYTES),
        }
    }
}

fn default_url() -> String {
    "nats://127.0.0.1:4222".to_string()
}

fn default_subject() -> String {
    "acts".to_string()
}

fn default_star() -> String {
    "*".to_string()
}

/// One engine event subscription, equivalent to the `MessageOptions` of a
/// gRPC `OnMessage` client: `type`/`state`/`uses`/`options` are the glob
/// filters of the engine channel.
#[derive(Debug, Clone, Deserialize)]
pub struct NatsChannelConfig {
    /// Subject this channel's events are published to. Defaults to
    /// `<config.subject>.evt.<id>` when unset.
    #[serde(default)]
    pub subject: Option<String>,
    /// Channel client id — engine delivery rows and acks are keyed by it.
    /// Defaults to the subject when unset.
    #[serde(default)]
    pub id: Option<String>,
    /// Glob pattern for the message type, e.g. `*` or `{act,msg}`.
    #[serde(default = "default_star")]
    pub r#type: String,
    /// Glob pattern for the message state, e.g. `*` or `{created,completed}`.
    #[serde(default = "default_star")]
    pub state: String,
    /// Glob pattern for the message uses, e.g. `acts.core.irq`.
    #[serde(default = "default_star")]
    pub uses: String,
    /// Custom key-glob filters applied to the message options.
    #[serde(default)]
    pub options: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults() {
        let cfg = NatsConfig::default();
        assert_eq!(cfg.url, "nats://127.0.0.1:4222");
        assert_eq!(cfg.subject, "acts");
        assert!(cfg.channels.is_empty());
        assert_eq!(cfg.max_payload_bytes(), DEFAULT_MAX_PAYLOAD_BYTES);
        assert!(
            !cfg.tls_required,
            "TLS is opt-in: a local dev broker is plaintext"
        );
        assert!(
            !cfg.reject_jetstream_capture,
            "the jetstream capture check is opt-in strictness"
        );
    }

    /// The raw payload cap is configured, defaulted when absent, and clamped
    /// to 1 KiB..64 MiB — a `0` (or anything smaller) is the floor, never an
    /// off switch: a cap that admits everything is no cap at all.
    #[test]
    fn max_payload_bytes_is_configured_defaulted_and_clamped() {
        let cfg: NatsConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(cfg.max_payload_bytes(), DEFAULT_MAX_PAYLOAD_BYTES);

        let cfg: NatsConfig = serde_json::from_value(json!({"max_payload_bytes": 4096})).unwrap();
        assert_eq!(cfg.max_payload_bytes(), 4096);

        let cfg: NatsConfig = serde_json::from_value(json!({"max_payload_bytes": 0})).unwrap();
        assert_eq!(cfg.max_payload_bytes(), 1024);
        let cfg: NatsConfig = serde_json::from_value(json!({"max_payload_bytes": 512})).unwrap();
        assert_eq!(cfg.max_payload_bytes(), 1024);

        let cfg: NatsConfig =
            serde_json::from_value(json!({"max_payload_bytes": u64::MAX as usize})).unwrap();
        assert_eq!(cfg.max_payload_bytes(), 64 * 1024 * 1024);
    }

    /// The strict startup knobs are plain booleans: set, they turn the broker
    /// TLS requirement and the jetstream capture check on.
    #[test]
    fn strict_start_flags_deserialize() {
        let cfg: NatsConfig =
            serde_json::from_value(json!({"tls_required": true, "reject_jetstream_capture": true}))
                .unwrap();
        assert!(cfg.tls_required);
        assert!(cfg.reject_jetstream_capture);
    }

    /// The in-flight bound is what the actions subscription is built with:
    /// configured, defaulted when absent or zero, and clamped so an absurd
    /// value cannot read as "no bound".
    #[test]
    fn max_in_flight_is_configured_defaulted_and_clamped() {
        assert_eq!(NatsConfig::default().max_in_flight(), DEFAULT_MAX_IN_FLIGHT);

        let cfg: NatsConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(cfg.max_in_flight(), DEFAULT_MAX_IN_FLIGHT);

        let cfg: NatsConfig = serde_json::from_value(json!({"max_in_flight": 0})).unwrap();
        assert_eq!(cfg.max_in_flight(), DEFAULT_MAX_IN_FLIGHT);

        let cfg: NatsConfig = serde_json::from_value(json!({"max_in_flight": 8})).unwrap();
        assert_eq!(cfg.max_in_flight(), 8);

        let cfg: NatsConfig = serde_json::from_value(json!({"max_in_flight": usize::MAX})).unwrap();
        assert_eq!(cfg.max_in_flight(), MAX_IN_FLIGHT);
    }

    /// A channel's filters default to `*` and its subject/id stay optional: a
    /// channel entry names only what it wants to override.
    #[test]
    fn channel_filters_default_to_star() {
        let channel: NatsChannelConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(channel.r#type, "*");
        assert_eq!(channel.state, "*");
        assert_eq!(channel.uses, "*");
        assert_eq!(channel.subject, None);
        assert_eq!(channel.id, None);
        assert!(channel.options.is_empty());
    }

    /// Broker credentials, url and subject are read from the section; a
    /// channel's options are the engine channel's key filters.
    #[test]
    fn credentials_and_channels_deserialize() {
        let cfg: NatsConfig = serde_json::from_value(json!({
            "url": "nats://broker:4222",
            "token": "secret",
            "username": "u",
            "password": "p",
            "subject": "prefix",
            "channels": [
                {"subject": "prefix.evt.a", "id": "a", "options": {"pid": "*"}},
                {},
            ],
        }))
        .unwrap();
        assert_eq!(cfg.url, "nats://broker:4222");
        assert_eq!(cfg.token.as_deref(), Some("secret"));
        assert_eq!(cfg.username.as_deref(), Some("u"));
        assert_eq!(cfg.password.as_deref(), Some("p"));
        assert_eq!(cfg.subject, "prefix");
        assert_eq!(cfg.channels.len(), 2);
        assert_eq!(cfg.channels[0].subject.as_deref(), Some("prefix.evt.a"));
        assert_eq!(cfg.channels[0].id.as_deref(), Some("a"));
        assert_eq!(
            cfg.channels[0].options.get("pid").map(String::as_str),
            Some("*")
        );
        assert_eq!(cfg.channels[1].subject, None);
        assert_eq!(cfg.channels[1].id, None);
    }
}
