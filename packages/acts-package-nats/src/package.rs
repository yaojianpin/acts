use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Result, Vars,
    include_json,
};
use async_nats::Client;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio::time::Instant;

const DATA_KEY: &str = "data";

/// How long one act may wait inside NATS when `[nats].timeout-ms` is unset: a
/// subscribe with no deadline waits for a message that may never be published
/// — a black-holed subject would hold the act, and its scheduler lane, until
/// the engine stops.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Largest `[nats].timeout-ms` accepted: the platform's ceiling on how long one
/// act may hold a lane while it waits for the broker.
pub const MAX_TIMEOUT_MS: u64 = 60 * 60 * 1000;

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Pub,
    Sub,
}

#[derive(Clone)]
pub struct NatsPackage {
    config: NatsConfig,
    client: Arc<OnceCell<Client>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NatsPackageParams {
    pub mode: Mode,
    pub subject: String,
    #[serde(default)]
    pub message: Option<JsonValue>,
}

#[derive(Clone, Deserialize)]
struct NatsConfig {
    url: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    /// Deadline of one act in milliseconds, in `1..=MAX_TIMEOUT_MS`; defaults
    /// to [`DEFAULT_TIMEOUT_MS`]. The engine config's `[nats]` section also
    /// carries the plugin's settings, which this lookup ignores.
    #[serde(default = "default_timeout_ms", rename = "timeout-ms")]
    timeout_ms: u64,
}

async fn connect(
    config: &NatsConfig,
) -> std::result::Result<Client, async_nats::error::Error<async_nats::ConnectErrorKind>> {
    let opts = if let Some(token) = &config.token {
        async_nats::ConnectOptions::with_token(token.clone())
    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        async_nats::ConnectOptions::with_user_and_password(user.clone(), pass.clone())
    } else {
        async_nats::ConnectOptions::default()
    };
    opts.connect(&config.url).await
}

#[async_trait::async_trait]
impl ActPackage for NatsPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.app.pubsub.nats",
            name: "Nats",
            desc: "publish or subscribe NATS messages",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 12h-4l-3 9L9 3l-3 9H2"/></svg>"#,
            doc: "",
            schema: include_json!("./schema.json"),
            options: Some(json!({
                "ui:order": ["mode", "subject", "message"],
                "message": {
                    "ui:widget": "textarea"
                }
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::App,
        }
    }

    fn new(config: &acts::Config) -> Result<Self>
    where
        Self: Sized,
    {
        let nats_config = config.get::<NatsConfig>("nats")?;
        // No value disables the deadline, and anything above the platform
        // ceiling fails at load instead of running a wait the deployment did
        // not ask for.
        if !(1..=MAX_TIMEOUT_MS).contains(&nats_config.timeout_ms) {
            return Err(ActError::Config(format!(
                "nats.timeout-ms must be between 1 and {MAX_TIMEOUT_MS} (got {})",
                nats_config.timeout_ms
            )));
        }
        Ok(Self {
            config: nats_config,
            client: Arc::new(OnceCell::new()),
        })
    }

    async fn execute(
        &self,
        ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        let params = serde_json::from_value::<NatsPackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

        // Everything this act waits on — the connection, the publish, the
        // subscription, the message — is bounded by the same deadline and by
        // the act's cancellation. `Ok(None)` is the cancelled case: the act
        // gave the wait up and reports no outcome of its own, because it did
        // not fail — whoever cancelled the task owns its state, and during a
        // shutdown the task stays running for the next start to resume.
        let cancel = ctx.cancellation_token();
        let timeout_ms = self.config.timeout_ms;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        let client = tokio::select! {
            client = self.client() => client?,
            _ = tokio::time::sleep_until(deadline) => return Err(timed_out(timeout_ms, "connect")),
            _ = cancel.cancelled() => return Ok(None),
        };

        match params.mode {
            Mode::Pub => {
                let payload = params.message.ok_or_else(|| {
                    ActError::Package("message is required for pub mode".to_string())
                })?;

                tokio::select! {
                    result = client.publish(params.subject, payload.to_string().into()) => {
                        result.map_err(|err| {
                            ActError::Package(format!("failed to publish message: {err}"))
                        })?;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        return Err(timed_out(timeout_ms, "publish"));
                    }
                    _ = cancel.cancelled() => return Ok(None),
                }
                Ok(None)
            }
            Mode::Sub => {
                // The subscription is closed with the act either way: a
                // cancelled or timed-out subscribe leaves no reader behind it.
                let mut sub = tokio::select! {
                    sub = client.subscribe(params.subject) => sub
                        .map_err(|err| ActError::Package(format!("failed to subscribe: {err}")))?,
                    _ = tokio::time::sleep_until(deadline) => {
                        return Err(timed_out(timeout_ms, "subscribe"));
                    }
                    _ = cancel.cancelled() => return Ok(None),
                };

                let msg = tokio::select! {
                    msg = sub.next() => msg.ok_or_else(|| {
                        ActError::Package("no message received".to_string())
                    })?,
                    _ = tokio::time::sleep_until(deadline) => {
                        return Err(timed_out(timeout_ms, "receive"));
                    }
                    _ = cancel.cancelled() => return Ok(None),
                };

                let data: JsonValue = serde_json::from_slice(&msg.payload)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&msg.payload).to_string().into());
                let mut ret = Vars::new();
                ret.set(DATA_KEY, data);
                Ok(Some(ret))
            }
        }
    }
}

/// The act's deadline ran out while it was `waiting` for the broker.
fn timed_out(timeout_ms: u64, waiting: &str) -> ActError {
    ActError::Package(format!(
        "nats {waiting} timed out after {timeout_ms} ms (timeout-ms)"
    ))
}

impl NatsPackage {
    async fn client(&self) -> Result<Client> {
        self.client
            .get_or_try_init(|| connect(&self.config))
            .await
            .map_err(|err| ActError::Config(format!("failed to connect to NATS: {err}")))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml_text: &str) -> acts::Config {
        acts::Config {
            data: Default::default(),
            table: toml::from_str::<toml::Table>(toml_text).unwrap(),
        }
    }

    /// The section is the NATS *plugin*'s as well, so the package tolerates
    /// everything else in it — including the plugin's channel tables — and
    /// reads only what it needs.
    #[test]
    fn the_broker_and_the_timeout_are_read_from_the_plugin_section() {
        let package = NatsPackage::new(&config(
            r#"
[nats]
url = "nats://localhost:4222"
subject = "acts"
max_in_flight = 256
timeout-ms = 1500

[[nats.channels]]
id = "c1"
subject = "acts.evt"
type = "*"
"#,
        ))
        .unwrap();
        assert_eq!(package.config.url, "nats://localhost:4222");
        assert_eq!(package.config.timeout_ms, 1500);

        // no timeout configured: the package's own default is in force
        let package =
            NatsPackage::new(&config("[nats]\nurl = \"nats://localhost:4222\"\n")).unwrap();
        assert_eq!(package.config.timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    /// No value disables the deadline, and anything above the platform ceiling
    /// fails at load rather than running a wait the deployment did not ask for.
    #[test]
    fn a_timeout_outside_the_platform_range_is_a_config_error() {
        for timeout_ms in [0, MAX_TIMEOUT_MS + 1] {
            let err = NatsPackage::new(&config(&format!(
                "[nats]\nurl = \"nats://localhost:4222\"\ntimeout-ms = {timeout_ms}\n"
            )))
            .err()
            .expect("a timeout outside the range must fail to load");
            assert!(
                matches!(err, ActError::Config(_)),
                "expected a config error, got {err:?}"
            );
        }

        // the inclusive ends are accepted
        NatsPackage::new(&config(&format!(
            "[nats]\nurl = \"nats://localhost:4222\"\ntimeout-ms = {MAX_TIMEOUT_MS}\n"
        )))
        .expect("the ceiling is a valid value");
    }
}
