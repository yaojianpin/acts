use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, CancellationToken,
    Context, Result, Vars, include_json,
};
use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use strum::AsRefStr;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    time::Instant,
};

const DATA_KEY: &str = "data";

/// Deadline of a shell act when `[shell].timeout-ms` is unset. A shell act
/// without a deadline holds its scheduler lane for as long as the script runs:
/// a script that waits forever takes an unbounded share of the engine's job
/// capacity with it.
pub const DEFAULT_TIMEOUT_MS: u64 = 5 * 60 * 1000;

/// Largest `[shell].timeout-ms` accepted: the platform's ceiling on how long
/// one act may hold a lane. A longer wait belongs in a workflow-level timeout
/// or a message act, not in a blocking shell act.
pub const MAX_TIMEOUT_MS: u64 = 60 * 60 * 1000;

/// Bytes captured from each stream (stdout and stderr separately) when
/// `[shell].max-output-bytes` is unset.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Largest `[shell].max-output-bytes` accepted. The capture is held in memory
/// and becomes the act's output vars, so it is bounded well below what the
/// machine could hold.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// How long a killed child is given to be reaped before the act gives up on
/// it. The wait is what removes the process-table entry; a wait that outlives
/// the grace would hold the act's lane for it, which is the failure this
/// package is bounding in the first place.
const REAP_GRACE_SECS: u64 = 5;

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
}

#[derive(Debug, Clone)]
pub struct ShellPackage {
    policy: ScriptPolicy,
    /// Deadline of one act, from `[shell].timeout-ms`.
    timeout_ms: u64,
    /// Bytes captured per stream, from `[shell].max-output-bytes`.
    max_output_bytes: usize,
}

/// Package-level `[shell]` configuration: what a workflow's script may be, and
/// how long it may run.
///
/// ```toml
/// [shell]
/// # when non-empty, only a script matching one of these may run
/// allow = ["ls", "ls *", "cat *.txt", "nu *"]
/// # always refused, allow or not
/// deny = ["*rm -rf*", "*sudo *", "*> /etc/*"]
/// # deadline of one shell act; defaults to 300000 (1..=3600000)
/// timeout-ms = 300000
/// # bytes captured per stream before the act fails; default 1048576
/// # (1..=67108864)
/// max-output-bytes = 1048576
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
/// `timeout-ms` and `max-output-bytes` bound one act's resources. They are the
/// deployment's decision and always in force — no value disables them, a value
/// outside the range is a startup error rather than a silent clamp, and an act
/// has no param that widens them.
///
/// **This is a policy, not a sandbox.** A glob over script text cannot see
/// what the script will do — `a=rm; $a -rf /` names no forbidden word, and a
/// script can do anything the server's own account may do that
/// `confine_script` does not name either. The lists are for making intent
/// explicit and for refusing the obvious, in the spirit of the workdir check
/// below them; a hostile workflow still needs an OS boundary (a container or a
/// namespace around the server).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ShellConfig {
    /// Script globs that may run. Empty means "anything not denied".
    pub allow: Vec<String>,
    /// Script globs that never run; wins over [`ShellConfig::allow`].
    pub deny: Vec<String>,
    /// Deadline of one shell act in milliseconds. `None` uses
    /// [`DEFAULT_TIMEOUT_MS`]; the accepted range is `1..=MAX_TIMEOUT_MS`.
    pub timeout_ms: Option<u64>,
    /// Bytes captured from each of stdout and stderr before the act fails.
    /// `None` uses [`DEFAULT_MAX_OUTPUT_BYTES`]; the accepted range is
    /// `1..=MAX_OUTPUT_BYTES`.
    pub max_output_bytes: Option<usize>,
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
                "ui:order": ["shell", "script", "content-type"],
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

    async fn execute(&self, ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        let mut ret = Vars::new();

        let params = serde_json::from_value::<ShellPackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

        // Both bounds are the deployment's `[shell]` section, resolved and
        // validated at load: a workflow cannot widen what the deployment
        // bounded, and there is no per-act value that leaves an act unbounded.
        let timeout_ms = self.timeout_ms;
        let max_output_bytes = self.max_output_bytes;

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
            .stderr(Stdio::piped())
            // Last-resort bound: if the act's future is dropped mid-flight
            // (runtime teardown), tokio kills the child and reaps it through
            // its orphan queue instead of leaving the script running.
            .kill_on_drop(true);
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

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let cancel = ctx.cancellation_token();
        let child = command
            .spawn()
            .map_err(|err| ActError::Package(format!("{err}")))?;
        // `None` when the act was cancelled: it gave up its child and reports
        // no outcome of its own (see `capture`).
        let Some(Captured {
            stdout,
            stderr,
            status,
        }) = capture(child, max_output_bytes, timeout_ms, deadline, &cancel).await?
        else {
            return Ok(None);
        };

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
            timeout_ms: bounded(
                config.timeout_ms,
                DEFAULT_TIMEOUT_MS,
                MAX_TIMEOUT_MS,
                "timeout-ms",
            )
            .map_err(ActError::Config)?,
            max_output_bytes: bounded(
                config.max_output_bytes,
                DEFAULT_MAX_OUTPUT_BYTES,
                MAX_OUTPUT_BYTES,
                "max-output-bytes",
            )
            .map_err(ActError::Config)?,
        })
    }
}

/// Resolve one bound: the act's (or the config's) value, or `default` when it
/// is absent. Zero and anything above the platform's `max` are refused, never
/// clamped — a bound that is silently not the one that was asked for is worse
/// than a loud error, and zero is exactly the unbounded value this package no
/// longer runs.
fn bounded<T>(value: Option<T>, default: T, max: T, field: &str) -> std::result::Result<T, String>
where
    T: Copy + Default + PartialOrd + std::fmt::Display,
{
    let value = value.unwrap_or(default);
    if value <= T::default() || value > max {
        return Err(format!(
            "shell {field} must be between 1 and {max} (got {value})"
        ));
    }
    Ok(value)
}

/// One finished shell act: both captured streams and the exit status.
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: ExitStatus,
}

/// Drive one shell act to a bounded end.
///
/// Every wait in here is bounded by the same absolute `deadline` and by the
/// act's cancellation, and none of them waits on anything else first:
///
/// - both pipes are drained concurrently — a script that fills the pipe of the
///   stream nobody reads blocks on write and never exits;
/// - a stream that hits the capture limit ends the act, and the child is
///   terminated instead of being waited for (a script that keeps writing would
///   otherwise never be waited up on);
/// - the wait for the child is bounded too, because closing both streams is
///   not the same as exiting.
///
/// On every path that does not end in a normal exit the child is killed and
/// reaped before this returns, so the act never leaves a process behind it and
/// never holds its scheduler lane waiting for one. `Ok(None)` is the cancelled
/// case: the child is gone and the act reports no outcome of its own.
async fn capture(
    mut child: Child,
    max_output_bytes: usize,
    timeout_ms: u64,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Option<Captured>> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ActError::Package("failed to capture shell stdout".to_string()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ActError::Package("failed to capture shell stderr".to_string()))?;

    // Both streams are drained at once, and the first failing one ends the
    // act: the other may stay silent until the deadline (a stream nobody
    // reads blocks the child on write, so it does not end by itself), and
    // waiting for it first is exactly the hang this bounds.
    let mut stdout_read = Box::pin(read_captured(
        &mut stdout,
        max_output_bytes,
        timeout_ms,
        deadline,
        cancel,
    ));
    let mut stderr_read = Box::pin(read_captured(
        &mut stderr,
        max_output_bytes,
        timeout_ms,
        deadline,
        cancel,
    ));
    let mut stdout_data: Option<Vec<u8>> = None;
    let mut stderr_data: Option<Vec<u8>> = None;
    while stdout_data.is_none() || stderr_data.is_none() {
        // the side travels with the outcome: the arms are otherwise identical
        let (stdout_side, outcome) = tokio::select! {
            result = &mut stdout_read, if stdout_data.is_none() => (true, result),
            result = &mut stderr_read, if stderr_data.is_none() => (false, result),
        };
        match outcome {
            Bounded::Done(data) => {
                if stdout_side {
                    stdout_data = Some(data);
                } else {
                    stderr_data = Some(data);
                }
            }
            Bounded::Cancelled => {
                terminate(&mut child).await;
                return Ok(None);
            }
            Bounded::Failed(err) => {
                terminate(&mut child).await;
                return Err(err);
            }
        }
    }
    let (stdout, stderr) = (
        stdout_data.expect("both streams are read to an outcome"),
        stderr_data.expect("both streams are read to an outcome"),
    );

    // Both streams reached EOF. The child normally exits with them; one that
    // closed its output and kept running does not, so its wait is bounded by
    // the same deadline.
    let exit = tokio::select! {
        status = child.wait() => Exit::Status(status),
        _ = tokio::time::sleep_until(deadline) => Exit::Deadline,
        _ = cancel.cancelled() => Exit::Cancelled,
    };

    match exit {
        Exit::Status(status) => Ok(Some(Captured {
            stdout,
            stderr,
            status: status.map_err(|err| ActError::Package(format!("{err}")))?,
        })),
        Exit::Deadline => {
            terminate(&mut child).await;
            Err(timed_out(timeout_ms))
        }
        Exit::Cancelled => {
            terminate(&mut child).await;
            Ok(None)
        }
    }
}

/// Why the wait for the child ended.
enum Exit {
    Status(std::io::Result<ExitStatus>),
    Deadline,
    Cancelled,
}

/// Kill the child and wait for it.
///
/// The wait is what removes the process-table entry, so a confirmed kill
/// leaves no zombie; the grace bounds that wait so a kill the OS refuses (or a
/// child that survives one) cannot hold the act open. Like every other bound
/// here, giving up is the point: the act fails, the lane is released.
async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(REAP_GRACE_SECS), child.wait()).await;
}

/// The act's deadline ran out.
fn timed_out(timeout_ms: u64) -> ActError {
    ActError::Package(format!(
        "shell command timed out after {timeout_ms} ms (timeout-ms)"
    ))
}

/// How one bounded wait of a shell act ended.
///
/// Cancellation is a third outcome rather than an error: the act did not fail,
/// it was stopped — the action that overrode the task owns the task's state,
/// and during a shutdown the task stays running so the next start resumes it.
/// Turning it into an error here would overwrite both.
enum Bounded<T> {
    Done(T),
    Failed(ActError),
    Cancelled,
}

async fn read_captured<R>(
    reader: &mut R,
    max_output_bytes: usize,
    timeout_ms: u64,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Bounded<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    let mut buf = [0_u8; 8 * 1024];

    loop {
        let size = tokio::select! {
            size = reader.read(&mut buf) => match size {
                Ok(size) => size,
                Err(err) => return Bounded::Failed(ActError::Package(format!("{err}"))),
            },
            _ = tokio::time::sleep_until(deadline) => {
                return Bounded::Failed(timed_out(timeout_ms));
            }
            _ = cancel.cancelled() => return Bounded::Cancelled,
        };
        if size == 0 {
            break;
        }

        if data.len() + size > max_output_bytes {
            return Bounded::Failed(ActError::Package(format!(
                "shell output stream exceeded max-output-bytes limit ({max_output_bytes})"
            )));
        }
        data.extend_from_slice(&buf[..size]);
    }

    Bounded::Done(data)
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
            ..Default::default()
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
            ..Default::default()
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid shell allow pattern"),
            "{err}"
        );

        let err = ScriptPolicy::new(&ShellConfig {
            deny: vec!["a{b".to_string()],
            ..Default::default()
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid shell deny pattern"),
            "{err}"
        );
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

    /// A deployment that says nothing still gets bounded acts: the package's
    /// own defaults are in force, never "no bound".
    #[test]
    fn a_silent_config_still_bounds_the_act() {
        let package = ShellPackage::from_config(&ShellConfig::default()).unwrap();
        assert_eq!(package.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(package.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
    }

    /// The `[shell]` section tunes both bounds, kebab-cased like the rest of
    /// the config.
    #[test]
    fn the_section_sets_both_bounds() {
        let config = acts::Config {
            data: Default::default(),
            table: toml::from_str::<toml::Table>(
                "[shell]\ntimeout-ms = 1500\nmax-output-bytes = 2048\n",
            )
            .unwrap(),
        };
        let package = ShellPackage::new(&config).unwrap();
        assert_eq!(package.timeout_ms, 1500);
        assert_eq!(package.max_output_bytes, 2048);
    }

    /// Zero (the unbounded value) and anything above the platform ceiling are
    /// startup errors, not a silent clamp to something else.
    #[test]
    fn a_bound_outside_the_platform_range_is_a_config_error() {
        for config in [
            ShellConfig {
                timeout_ms: Some(0),
                ..Default::default()
            },
            ShellConfig {
                timeout_ms: Some(MAX_TIMEOUT_MS + 1),
                ..Default::default()
            },
            ShellConfig {
                max_output_bytes: Some(0),
                ..Default::default()
            },
            ShellConfig {
                max_output_bytes: Some(MAX_OUTPUT_BYTES + 1),
                ..Default::default()
            },
        ] {
            let err = ShellPackage::from_config(&config).unwrap_err();
            assert!(
                matches!(err, ActError::Config(_)),
                "expected a config error, got {err:?}"
            );
        }

        // The inclusive ends are accepted.
        let package = ShellPackage::from_config(&ShellConfig {
            timeout_ms: Some(MAX_TIMEOUT_MS),
            max_output_bytes: Some(MAX_OUTPUT_BYTES),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(package.timeout_ms, MAX_TIMEOUT_MS);
        assert_eq!(package.max_output_bytes, MAX_OUTPUT_BYTES);
    }
}
