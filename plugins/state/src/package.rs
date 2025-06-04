use acts::{
    ActError, ActOperation, ActPackage, ActPackageCatalog, ActPackageMeta, ActResource, ActRunAs,
    Result, Vars,
};
use serde::Serialize;
use serde_json::json;

#[derive(Clone, Serialize)]
pub struct StatePackage {
    op: String,
    params: Vars,
}

impl ActPackage for StatePackage {
    fn meta() -> acts::ActPackageMeta {
        ActPackageMeta {
            name: "acts.app.state",
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
            run_as: ActRunAs::Irq,
            resources: vec![ActResource {
                name: "Get or set state store".to_string(),
                desc: "get or set a state from the state store".to_string(),
                operations: vec![
                    ActOperation {
                        name: "GET state".to_string(),
                        desc: "get a state from the state store".to_string(),
                        value: "GET".to_string(),
                    },
                    ActOperation {
                        name: "SET state".to_string(),
                        desc: "set a state to the state store".to_string(),
                        value: "SET".to_string(),
                    },
                ],
            }],
            catalog: ActPackageCatalog::App,
        }
    }
}

impl<'de> serde::de::Deserialize<'de> for StatePackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let params: Vars = Vars::deserialize(deserializer)?;
        let op = params
            .get::<String>("op")
            .ok_or(serde::de::Error::custom("missing 'op' in params"))?
            .to_string();

        Ok(Self { op, params })
    }
}

impl StatePackage {
    pub fn run(&self, client: &redis::Client, pid: &str) -> Result<Vars> {
        let mut conn = client.get_connection().map_err(|err| {
            ActError::Package(format!("error happend to get connection: {}", err))
        })?;
        match self.op.as_str() {
            "GET" => {
                let key = self
                    .params
                    .get::<String>("key")
                    .ok_or(ActError::Package("missing 'key' in params".to_string()))?
                    .to_string();

                let ret = redis::cmd("GET")
                    .arg(&format!("{pid}:{key}"))
                    .query::<String>(&mut conn)
                    .map_err(|err| {
                        ActError::Package(format!("error happend to set value: {}", err))
                    })?;

                let mut vars = Vars::new();
                vars.insert(
                    key,
                    serde_json::from_str(&ret).map_err(|err| {
                        ActError::Package(format!("error happend to parse value: {}", err))
                    })?,
                );

                Ok(vars)
            }
            "SET" => {
                let key = self
                    .params
                    .get::<String>("key")
                    .ok_or(ActError::Package("missing 'key' in params".to_string()))?
                    .to_string();

                let value: serde_json::Value = self
                    .params
                    .get("value")
                    .ok_or(ActError::Package("missing 'value' in params".to_string()))?;

                let v = serde_json::to_string(&value).map_err(|err| {
                    ActError::Package(format!("error happend to parse value: {}", err))
                })?;

                redis::cmd("SET")
                    .arg(&&format!("{pid}:{key}"))
                    .arg(v.as_str())
                    .query::<String>(&mut conn)
                    .map_err(|err| {
                        ActError::Package(format!("error happend to set value: {}", err))
                    })?;
                Ok(Vars::new())
            }
            _ => Err(ActError::Package(format!("invalid operation: {}", self.op))),
        }
    }
}
