use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageMeta, ActRunAs, Result, Vars, include_json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::process::Command;
use strum::AsRefStr;

const DATA_KEY: &str = "data";

#[derive(Debug, Clone, Deserialize, Serialize, AsRefStr)]
pub enum Shell {
    #[allow(clippy::enum_variant_names)]
    #[serde(rename(deserialize = "nu"))]
    #[strum(serialize = "nu")]
    NuShell,
    #[serde(rename(deserialize = "bash"))]
    #[strum(serialize = "bash")]
    Bash,
    #[allow(clippy::enum_variant_names)]
    #[serde(rename(deserialize = "powershell"))]
    #[strum(serialize = "powershell")]
    PowerShell,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum ContentType {
    #[serde(rename(deserialize = "text"))]
    Text,
    #[serde(rename(deserialize = "json"))]
    Json,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShellPackage {
    shell: Shell,
    script: String,
    #[serde(rename(deserialize = "content-type"))]
    content_type: Option<ContentType>,
}

impl ActPackage for ShellPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.app.shell",
            desc: "do shell script with nushell, bash or powershell",
            version: "0.1.0",
            icon: "icon-shell",
            doc: "",
            schema: include_json!("./schema.json"),
            run_as: ActRunAs::Irq,
            resources: vec![],
            catalog: ActPackageCatalog::App,
        }
    }
}

impl ShellPackage {
    pub fn create(inputs: &Vars) -> Result<Self> {
        let params = inputs
            .get::<serde_json::Value>("params")
            .ok_or(ActError::Package("missing 'params' in package".to_string()))?;
        let package = serde_json::from_value::<Self>(params)?;
        Ok(package)
    }

    pub async fn run(&self) -> Result<Vars> {
        let mut ret = Vars::new();
        let output = Command::new(self.shell.as_ref())
            .arg("-c")
            .arg(&self.script)
            .output()
            .map_err(|err| ActError::Package(format!("{}", err)))?;

        if !output.status.success() {
            let err = String::from_utf8(output.stderr)?;
            return Err(ActError::Package(err));
        }
        let data = String::from_utf8(output.stdout)?;
        let content_type = self.content_type.as_ref().unwrap_or(&ContentType::Text);
        match content_type {
            ContentType::Text => ret.set(DATA_KEY, data),
            ContentType::Json => ret.set(
                DATA_KEY,
                serde_json::from_str::<JsonValue>(&data).map_err(|err| {
                    ActError::Package(format!("failed to convert data to json: {}", err))
                })?,
            ),
        }

        Ok(ret)
    }
}
