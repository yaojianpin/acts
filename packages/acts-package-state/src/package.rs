use crate::config::StateConfig;
use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActResource, ActRunAs, Result,
    Vars,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::OnceCell;

const CONFIG_NAME: &str = "state";

#[derive(Clone)]
pub struct StatePackage {
    client: redis::Client,
    connection: Arc<OnceCell<redis::aio::MultiplexedConnection>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatePackageParams {
    op: String,
    params: Vars,
}

#[async_trait::async_trait]
impl ActPackage for StatePackage {
    fn definition() -> acts::ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.app.state",
            name: "State",
            desc: "get or set state to redis",
            version: "0.1.0",
            icon: "icon-app-state",
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["GET", "SET" ] },
                    "key": { "type": "string" },
                    "value": { "type": ["number", "string", "boolean", "array", "object"] },
                },
                "required": ["op", "key"],
            }),
            options: Some(json!({
                "ui:order": ["op", "key", "value"],
                "op": {
                    "ui:widget": "select",
                    "ui:options": {
                        "label": false,
                        "placeholder": "Select an operation"
                    }
                },
                "key": {
                    "ui:widget": "text",
                    "ui:options": {
                        "label": false,
                        "placeholder": "Enter the key"
                    }
                },
                "value": {
                    "ui:widget": "textarea",
                    "ui:options": {
                        "label": false,
                        "placeholder": "Enter the value"
                    }
                }
            })),
            run_as: ActRunAs::Func,
            resources: vec![
                ActResource {
                    name: "Get state store".to_string(),
                    desc: "get a state from the state store".to_string(),
                    value: json!({ "op": "GET"}),
                },
                ActResource {
                    name: "Set state store".to_string(),
                    desc: "set a state from the state store".to_string(),
                    value: json!({ "op": "SET"}),
                },
            ],
            catalog: ActPackageCatalog::App,
        }
    }

    fn new(config: &acts::Config) -> Result<Self>
    where
        Self: Sized,
    {
        if !config.has(CONFIG_NAME) {
            return Err(acts::ActError::Config(
                "missing 'state' section in config file".to_string(),
            ));
        }
        let config = config
            .get::<StateConfig>(CONFIG_NAME)
            .map_err(|err| acts::ActError::Config(format!("get state config error: {err}")))?;

        let client = redis::Client::open(config.database_uri.as_str())
            .map_err(|err| acts::ActError::Config(format!("create redis client error: {err}")))?;

        Ok(Self {
            client,
            connection: Arc::new(OnceCell::new()),
        })
    }

    async fn execute(
        &self,
        ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        let mut conn = self.connection().await?;

        let pid = ctx.task().pid.to_string();
        let params = serde_json::from_value::<StatePackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;
        match params.op.as_str() {
            "GET" => {
                let key = params
                    .params
                    .get::<String>("key")
                    .ok_or(ActError::Package("missing 'key' in params".to_string()))?
                    .to_string();

                let ret = redis::cmd("GET")
                    .arg(format!("{pid}:{key}"))
                    .query_async::<String>(&mut conn)
                    .await
                    .map_err(|err| {
                        ActError::Package(format!("error happend to get value: {err}"))
                    })?;

                let mut vars = Vars::new();
                vars.insert(
                    key,
                    serde_json::from_str(&ret).map_err(|err| {
                        ActError::Package(format!("error happend to parse value: {err}"))
                    })?,
                );

                Ok(Some(vars))
            }
            "SET" => {
                let key = params
                    .params
                    .get::<String>("key")
                    .ok_or(ActError::Package("missing 'key' in params".to_string()))?
                    .to_string();

                let value: serde_json::Value = params
                    .params
                    .get("value")
                    .ok_or(ActError::Package("missing 'value' in params".to_string()))?;

                let v = serde_json::to_string(&value).map_err(|err| {
                    ActError::Package(format!("error happend to parse value: {err}"))
                })?;

                redis::cmd("SET")
                    .arg(format!("{pid}:{key}"))
                    .arg(v.as_str())
                    .query_async::<String>(&mut conn)
                    .await
                    .map_err(|err| {
                        ActError::Package(format!("error happend to set value: {err}"))
                    })?;
                Ok(None)
            }
            _ => Err(ActError::Package(format!(
                "invalid operation: {}",
                params.op
            ))),
        }
    }
}

impl StatePackage {
    async fn connection(&self) -> Result<redis::aio::MultiplexedConnection> {
        let client = self.client.clone();
        let connection = self
            .connection
            .get_or_try_init(|| open_connection(client))
            .await?;

        Ok(connection.clone())
    }
}

async fn open_connection(client: redis::Client) -> Result<redis::aio::MultiplexedConnection> {
    let mut connection = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|err| ActError::Package(format!("error happend to connect redis: {err}")))?;

    redis::cmd("PING")
        .query_async::<String>(&mut connection)
        .await
        .map_err(|err| ActError::Package(format!("error happend to ping redis: {err}")))?;

    Ok(connection)
}
