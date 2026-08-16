use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Result, Vars,
    include_json,
};
use async_nats::Client;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

const DATA_KEY: &str = "data";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Pub,
    Sub,
}

#[derive(Debug, Clone)]
pub struct NatsPackage {
    client: Client,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NatsPackageParams {
    pub mode: Mode,
    pub subject: String,
    #[serde(default)]
    pub message: Option<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
struct NatsConfig {
    url: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
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
        let client = tokio::runtime::Handle::current()
            .block_on(connect(&nats_config))
            .map_err(|err| ActError::Config(format!("failed to connect to NATS: {err}")))?;

        Ok(Self { client })
    }

    async fn execute(
        &self,
        _ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        let params = serde_json::from_value::<NatsPackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

        match params.mode {
            Mode::Pub => {
                let payload = params.message.ok_or_else(|| {
                    ActError::Package("message is required for pub mode".to_string())
                })?;

                self.client
                    .publish(params.subject, payload.to_string().into())
                    .await
                    .map_err(|err| {
                        ActError::Package(format!("failed to publish message: {err}"))
                    })?;

                Ok(None)
            }
            Mode::Sub => {
                let mut sub = self
                    .client
                    .subscribe(params.subject)
                    .await
                    .map_err(|err| ActError::Package(format!("failed to subscribe: {err}")))?;

                let msg = sub
                    .next()
                    .await
                    .ok_or_else(|| ActError::Package("no message received".to_string()))?;

                let mut ret = Vars::new();
                let data: JsonValue = serde_json::from_slice(&msg.payload)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&msg.payload).to_string().into());
                ret.set(DATA_KEY, data);
                Ok(Some(ret))
            }
        }
    }
}
