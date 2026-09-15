use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Result, Vars,
    include_json,
};
use globset::{GlobBuilder, GlobMatcher};
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

#[derive(Debug, Clone)]
pub struct ShellPackage {
    policy: ScriptPolicy,
}

/// Package-level `[shell]` configuration: what a workflow's script may be.
///
/// ```toml
/// [shell]
/// # when non-empty, only a script matching one of these may run
/// allow = ["ls", "ls *", "cat *.txt", "nu *"]
/// # always refused, allow or not
/// deny = ["*rm -rf*", "*sudo *", "*> /etc/*"]
/// ```
///
/// Patterns are globs over the **whole script text** — `*` matches any run of
/// characters, newlines and `/` included, `?` matches one, `[abc]` one of a
/// set — so `rm *` matches a script that *starts* with `rm` and `*rm *`
/// matches one that contains it anywhere. Matching the script rather than a
/// parsed command is deliberate: the package does not parse a shell (that is
/// the shell's job, and no two shells agree), so the rule is the one thing it
/// can state exactly — "this text, or not".
///
/// `deny` wins over `allow`. Both lists empty means no restriction, which is
/// the behaviour of a deployment that says nothing; the moment either is
/// written, the policy is the judgement. A pattern that does not compile is a
/// startup error, never a silent allow: a policy that cannot be enforced must
/// not run.
///
/// **This is a policy, not a sandbox.** A glob over script text cannot see
/// what the script will do — `a=rm; $a -rf /` names no forbidden word, and a
/// script can do anything the server's own account may do that
/// `confine_script` does not name either. The lists are for making intent
/// explicit and for refusing the obvious, in the spirit of the workdir check
/// below them; a hostile workflow still needs an OS boundary (a container or a
/// namespace around the server).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Script globs that may run. Empty means "anything not denied".
    pub allow: Vec<String>,
    /// Script globs that never run; wins over [`ShellConfig::allow`].
    pub deny: Vec<String>,
}

/// The compiled [`ShellConfig`]: allow/deny globs, deny first.
#[derive(Debug, Clone, Default)]
pub struct ScriptPolicy {
    allow: Vec<GlobMatcher>,
    deny: Vec<GlobMatcher>,
}

impl ScriptPolicy {
    /// Compile the configured globs. A malformed pattern is an error here,
    /// where it fails startup, rather than at the first script it would have
    /// governed.
    pub fn new(config: &ShellConfig) -> Result<Self> {
        Ok(Self {
            allow: compile(&config.allow, "allow")?,
            deny: compile(&config.deny, "deny")?,
        })
    }

    /// Whether `script` may run: not denied, and — when an allow list exists —
    /// matched by it.
    pub fn allows(&self, script: &str) -> bool {
        if self.deny.iter().any(|glob| glob.is_match(script)) {
            return false;
        }
        self.allow.is_empty() || self.allow.iter().any(|glob| glob.is_match(script))
    }
}

/// Compile one pattern list. `literal_separator(false)` is what makes `*`
/// mean "any characters" rather than "any characters but `/`": a script is one
/// string, not a path, and `cat *` has to match `cat sub/dir/file.txt`.
fn compile(patterns: &[String], field: &str) -> Result<Vec<GlobMatcher>> {
    patterns
        .iter()
        .map(|pattern| {
            GlobBuilder::new(pattern)
                .literal_separator(false)
                .build()
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    ActError::Config(format!("invalid shell {field} pattern '{pattern}': {err}"))
                })
        })
        .collect()
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
    fn new(config: &acts::Config) -> Result<Self>
    where
        Self: Sized,
    {
        let config = if config.has("shell") {
            config.get::<ShellConfig>("shell")?
        } else {
            ShellConfig::default()
        };
        Self::from_config(&config)
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

        // The `[shell]` allow/deny lists, checked before anything is spawned:
        // a script the deployment did not admit never reaches the shell.
        if !self.policy.allows(&params.script) {
            return Err(ActError::Package(format!(
                "the script is refused by the [shell] policy: it is not admitted by `allow` \
                 or it matches `deny` ({} characters)",
                params.script.len()
            )));
        }

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

impl ShellPackage {
    /// Build the package from an explicit `[shell]` config, bypassing the
    /// engine config lookup.
    pub fn from_config(config: &ShellConfig) -> Result<Self> {
        Ok(Self {
            policy: ScriptPolicy::new(config)?,
        })
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
    fn compile_policy(allow: &[&str], deny: &[&str]) -> ScriptPolicy {
        ScriptPolicy::new(&ShellConfig {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        })
        .expect("compile policy")
    }

    /// An empty policy does not restrict: a deployment that lists nothing has
    /// said nothing, and the check is the deployment's to make.
    #[test]
    fn an_empty_policy_admits_every_script() {
        let policy = compile_policy(&[], &[]);
        for script in ["ls", "rm -rf /", "curl http://example.com", "a\nb\nc"] {
            assert!(policy.allows(script), "should be allowed: {script}");
        }
    }

    /// A non-empty allow list is the whole of what may run — anything else is
    /// refused, not merely unmatched.
    #[test]
    fn a_non_empty_allow_list_is_exhaustive() {
        let policy = compile_policy(&["ls", "ls *", "cat *.txt"], &[]);
        for script in ["ls", "ls -la /tmp", "cat notes.txt"] {
            assert!(policy.allows(script), "should be allowed: {script}");
        }
        for script in ["rm -rf /", "cat notes.md", "ls; rm -rf /", "  ls"] {
            assert!(!policy.allows(script), "should be refused: {script}");
        }
    }

    /// `*` spans `/` and newlines: a script is one string, not a path, so
    /// `cat *` has to reach `cat sub/dir/file.txt`.
    #[test]
    fn a_star_matches_across_separators_and_lines() {
        let policy = compile_policy(&["cat *"], &[]);
        assert!(policy.allows("cat sub/dir/file.txt"));
        assert!(policy.allows("cat a\ncat b"));

        let policy = compile_policy(&["nu *"], &[]);
        assert!(policy.allows("nu -c 'echo hi'"));
    }

    /// Deny wins over allow, contains anywhere in the script, and is checked
    /// first — a script both listed and forbidden is refused.
    #[test]
    fn deny_wins_over_allow() {
        let policy = compile_policy(&["ls *"], &["*rm -rf*", "*sudo *"]);
        assert!(policy.allows("ls -la"));
        assert!(!policy.allows("rm -rf /"));
        assert!(!policy.allows("ls\nrm -rf /"));
        assert!(!policy.allows("ls; sudo reboot"));

        let policy = compile_policy(&["*rm -rf*"], &["*rm -rf*"]);
        assert!(!policy.allows("rm -rf /"));

        // deny is effective with no allow list at all
        let policy = compile_policy(&[], &["*rm -rf*"]);
        assert!(policy.allows("ls"));
        assert!(!policy.allows("cd /tmp && rm -rf *"));
    }

    /// A pattern that does not compile fails at load, where it is a startup
    /// error, instead of silently governing nothing.
    #[test]
    fn an_invalid_pattern_is_a_config_error() {
        let err = ScriptPolicy::new(&ShellConfig {
            allow: vec!["ls [unclosed".to_string()],
            deny: vec![],
        })
        .unwrap_err();
        assert!(err.to_string().contains("invalid shell allow pattern"), "{err}");

        let err = ScriptPolicy::new(&ShellConfig {
            allow: vec![],
            deny: vec!["a{b".to_string()],
        })
        .unwrap_err();
        assert!(err.to_string().contains("invalid shell deny pattern"), "{err}");
    }

    /// The config lookup is the `[shell]` section, and a section with no lists
    /// is no restriction.
    #[test]
    fn the_section_is_read_from_the_engine_config() {
        let config = acts::Config {
            data: Default::default(),
            table: toml::from_str::<toml::Table>(
                "[shell]\nallow = [\"ls *\"]\ndeny = [\"*rm *\"]\n",
            )
            .unwrap(),
        };
        let package = ShellPackage::new(&config).unwrap();
        assert!(package.policy.allows("ls -la"));
        assert!(!package.policy.allows("rm file"));
        assert!(!package.policy.allows("echo hi"));

        // No section: nothing is restricted.
        let package = ShellPackage::new(&acts::Config::default()).unwrap();
        assert!(package.policy.allows("anything at all"));
    }
}
