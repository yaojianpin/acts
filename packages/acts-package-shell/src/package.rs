use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Result, Vars,
    include_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::process::Stdio;
use strum::AsRefStr;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const DATA_KEY: &str = "data";

#[derive(Debug, Clone, Deserialize, Serialize, AsRefStr)]
pub enum Shell {
    #[serde(rename(deserialize = "sh"))]
    #[strum(serialize = "sh")]
    Sh,
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

#[derive(Debug, Clone)]
pub struct ShellPackage;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShellPackageParams {
    shell: Option<Shell>,
    script: String,
    #[serde(rename(deserialize = "content-type"))]
    content_type: Option<ContentType>,
    /// Optional maximum number of bytes for each captured output stream. This
    /// is opt-in to avoid changing the behavior of existing workflows.
    #[serde(default, rename(deserialize = "max-output-bytes"))]
    max_output_bytes: Option<usize>,
}

#[async_trait::async_trait]
impl ActPackage for ShellPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.app.shell",
            name: "Shell",
            desc: "do shell script with nushell, bash or powershell",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-square-chevron-right-icon lucide-square-chevron-right"><rect width="18" height="18" x="3" y="3" rx="2"/><path d="m10 8 4 4-4 4"/></svg>"#,
            doc: "",
            schema: include_json!("./schema.json"),
            options: Some(json!({
                "ui:order": ["shell", "script", "content-type", "max-output-bytes"],
                "script": {
                    "ui:widget": "textarea",
                },
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::App,
        }
    }
    fn new(_: &acts::Config) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    async fn execute(
        &self,
        _ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        let mut ret = Vars::new();

        let params = serde_json::from_value::<ShellPackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

        let shell = params.shell.as_ref().unwrap_or(&Shell::Sh);
        let mut child = Command::new(shell.as_ref())
            .arg("-c")
            .arg(&params.script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| ActError::Package(format!("{err}")))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ActError::Package("failed to capture shell stdout".to_string()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| ActError::Package("failed to capture shell stderr".to_string()))?;
        let (stdout_data, stderr_data, status) = tokio::join!(
            read_captured(&mut stdout, params.max_output_bytes),
            read_captured(&mut stderr, params.max_output_bytes),
            child.wait(),
        );
        let stdout = stdout_data?;
        let stderr = stderr_data?;
        let status = status.map_err(|err| ActError::Package(format!("{err}")))?;

        if !status.success() {
            let err = String::from_utf8(stderr)?;
            return Err(ActError::Package(err));
        }
        let data = String::from_utf8(stdout)?;
        let content_type = params.content_type.as_ref().unwrap_or(&ContentType::Text);
        match content_type {
            ContentType::Text => ret.set(DATA_KEY, data),
            ContentType::Json => ret.set(
                DATA_KEY,
                serde_json::from_str::<JsonValue>(&data).map_err(|err| {
                    ActError::Package(format!("failed to convert data to json: {err}"))
                })?,
            ),
        }

        Ok(Some(ret))
    }
}

async fn read_captured<R>(reader: &mut R, max_output_bytes: Option<usize>) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    let mut captured_size = 0_usize;
    let mut buf = [0_u8; 8 * 1024];

    loop {
        let size = reader.read(&mut buf).await?;
        if size == 0 {
            break;
        }

        if let Some(max_output_bytes) = max_output_bytes {
            captured_size += size;
            if captured_size > max_output_bytes {
                return Err(ActError::Package(format!(
                    "shell output stream exceeded max-output-bytes limit ({max_output_bytes})"
                )));
            }
        }
        data.extend_from_slice(&buf[..size]);
    }

    Ok(data)
}
