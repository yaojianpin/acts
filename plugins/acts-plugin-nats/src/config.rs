use serde::Deserialize;
use std::collections::HashMap;

/// How many inbound actions may execute at once when `[nats].max_in_flight`
/// does not say otherwise.
pub const DEFAULT_MAX_IN_FLIGHT: usize = 256;

/// Upper bound of `[nats].max_in_flight`: an action bound this high is already
/// no bound.
const MAX_IN_FLIGHT: usize = 65_536;

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
}
