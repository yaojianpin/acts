use serde::Deserialize;
use std::collections::HashMap;

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
            channels: Vec::new(),
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

    #[test]
    fn defaults() {
        let cfg = NatsConfig::default();
        assert_eq!(cfg.url, "nats://127.0.0.1:4222");
        assert_eq!(cfg.subject, "acts");
        assert!(cfg.channels.is_empty());
    }
}
