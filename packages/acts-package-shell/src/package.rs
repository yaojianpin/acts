use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Result, Vars,
    include_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::path::Path;
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
        ctx: &acts::Context,
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

        // Directory control: when the engine's ACL config gives this process a
        // workdir, the script runs inside it and may not name a path outside.
        // See `confine_script` for what that check can and cannot catch.
        let workdir = ctx.workdir();
        if let Some(dir) = &workdir {
            confine_script(&params.script, dir)?;
        }

        let shell = params.shell.as_ref().unwrap_or(&Shell::Sh);
        let mut command = Command::new(shell.as_ref());
        command
            .arg("-c")
            .arg(&params.script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &workdir {
            // The working directory confines relative paths; the home and temp
            // variables keep the tools that default to them inside too, and
            // `ACTS_WORKDIR` gives a script an explicit handle on its own
            // directory.
            command
                .current_dir(dir)
                .env("HOME", dir)
                .env("PWD", dir)
                .env("TMPDIR", dir)
                .env("TEMP", dir)
                .env("TMP", dir)
                .env(WORKDIR_ENV, dir);
        }
        let mut child = command
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

/// Environment variable naming the process workdir, so a script can address
/// its own directory without hardcoding a path (and without leaving it).
const WORKDIR_ENV: &str = "ACTS_WORKDIR";

/// Refuse a script that names a path outside `workdir`.
///
/// The **containment** is the child's working directory (plus `HOME`/`TMPDIR`
/// pointing inside it): relative paths resolve inside the workdir, and that is
/// what the process actually gets. This check is the additional *policy*
/// layer — it turns the direct escape into a loud refusal instead of a silent
/// success, and it is what makes "may not touch the rest of the filesystem"
/// visible in a workflow's own error rather than in an audit.
///
/// It rejects, token by token over the script text: an absolute path
/// (`/etc/passwd`, `C:\Windows`, `\\server\share`) and a `..` path segment
/// (`../secrets`, `/tmp/../../etc`). It is deliberately NOT a security
/// boundary on its own — a shell can spell a path in ways no textual check
/// can follow (`a=/etc; cat $a/passwd`, `file:///etc/passwd`, a symlink
/// inside the workdir, `$PWD/../..`) — so it is documented as best-effort:
/// quotes, splitting and metacharacters are not interpreted, and a script
/// that names an outside path in a way this misses is caught by nothing else
/// here. A real boundary is an OS one (a container or a namespace sandbox
/// around the server), which is where the workdir being per-process helps.
fn confine_script(script: &str, workdir: &Path) -> Result<()> {
    for token in script.split(|c: char| {
        c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '<' | '>' | '"' | '\'')
    }) {
        let escapes = is_absolute_path(token) || has_parent_segment(token);
        if escapes {
            return Err(ActError::Package(format!(
                "script names '{token}', outside this run's directory {} (ACTS_WORKDIR); \
                 the process workdir confines every relative path, so refer to files it \
                 contains",
                workdir.display()
            )));
        }
    }
    Ok(())
}

/// A token that is an absolute path on either platform. A URL's `//` is not
/// one (`http://host` does not start with a separator), which is intended:
/// network access is not the filesystem's business here — an API with a path
/// (`http://host/a`) is likewise left to the act that performs the request.
fn is_absolute_path(token: &str) -> bool {
    if token.starts_with('/') || token.starts_with('\\') {
        return true;
    }
    // Windows drive or UNC form: `C:\dir`, `C:/dir`, `\\host\share`.
    matches!(
        token.as_bytes(),
        [drive, b':', ..] if drive.is_ascii_alphabetic()
    )
}

/// A token with a `..` path segment — a traversal whichever platform's
/// separators it uses. `..` inside a longer name (`a..b`) is not one.
fn has_parent_segment(token: &str) -> bool {
    token
        .split(['/', '\\'])
        .any(|segment| segment.trim() == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(script: &str) -> Result<()> {
        confine_script(script, Path::new("/work/pid1"))
    }

    #[test]
    fn confined_script_allows_relative_work_inside_the_workdir() {
        for script in [
            "echo hello",
            "./run.sh --flag",
            "cat sub/dir/file.txt",
            "cp a.txt b.txt",
            "sed -e 's/a/b/' data.txt",
            "grep -rn todo src",
            "ls",
            "printf '%s' \"$ACTS_WORKDIR\"",
            "tar -czf out.tgz .",
            "a..b/c..d",
        ] {
            assert!(check(script).is_ok(), "should be allowed: {script}");
        }
    }

    #[test]
    fn confined_script_rejects_absolute_paths() {
        for script in [
            "cat /etc/passwd",
            "ls /tmp",
            "sh /opt/x.sh",
            "cat C:\\Windows\\win.ini",
            "cat c:/Users/me/.ssh/id_rsa",
            "type \\\\server\\share\\f",
            "cat '/etc/shadow'",
            "> /etc/hosts",
        ] {
            let err = check(script).expect_err(script).to_string();
            assert!(
                err.contains("outside this run's directory"),
                "unexpected error for {script}: {err}"
            );
        }
    }

    #[test]
    fn confined_script_rejects_parent_traversal() {
        for script in [
            "cat ../secrets",
            "cat sub/../../etc/passwd",
            "cd .. && ls",
            "cat ..\\secrets",
            "cp x ../../out",
        ] {
            assert!(check(script).is_err(), "should be refused: {script}");
        }
    }

    #[test]
    fn a_url_is_not_read_as_a_path() {
        // The guard is about the filesystem, not the network.
        assert!(check("curl http://example.com/a/b").is_ok());
    }
}
