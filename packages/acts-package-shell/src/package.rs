use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Context, Result, Vars,
    include_json,
};
use bashkit::{
    Bash, Error as BashError, ExecutionLimits, LimitExceeded, PosixFs, RealFs, RealFsMode,
};
use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::sync::Arc;
use std::time::Duration;

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

/// Environment variable naming the run's directory as the script sees it: the
/// root of the interpreter's filesystem, `/`. A script that wants to address
/// its own directory explicitly reads this instead of hardcoding a path —
/// there is no host path to hardcode, the root is the run's directory.
const WORKDIR_ENV: &str = "ACTS_WORKDIR";

/// The interpreter a script runs under.
///
/// Bashkit's virtual bash is the only one the package runs, so `bash` is the
/// only value: a workflow whose params name `sh`, `nu` or `powershell` fails
/// to deserialize instead of being quietly run as bash.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Shell {
    #[serde(rename = "bash")]
    Bash,
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
/// allow = ["ls", "ls *", "cat *.txt"]
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
/// parsed command is deliberate: the package does not parse bash (that is the
/// interpreter's job), so the rule is the one thing it can state exactly —
/// "this text, or not".
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
/// what the script will do — `a=rm; $a -rf /` names no forbidden word. The
/// sandbox is the interpreter's filesystem, which is the run's own directory
/// and nothing else; the lists are for making intent explicit and for refusing
/// the obvious before anything runs.
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
            desc: "run a bash script in the run's own directory",
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

        // The interpreter the act runs under. The enum admits no other value,
        // so a script that names `sh`, `nu` or `powershell` failed to
        // deserialize above instead of being run as bash.
        let Shell::Bash = params.shell.unwrap_or(Shell::Bash);

        // Both bounds are the deployment's `[shell]` section, resolved and
        // validated at load: a workflow cannot widen what the deployment
        // bounded, and there is no per-act value that leaves an act unbounded.
        let timeout_ms = self.timeout_ms;
        let max_output_bytes = self.max_output_bytes;

        // The `[shell]` allow/deny lists, checked before anything is run: a
        // script the deployment did not admit never reaches the interpreter.
        if !self.policy.allows(&params.script) {
            return Err(ActError::Package(format!(
                "the script is refused by the [shell] policy: it is not admitted by `allow` \
                 or it matches `deny` ({} characters)",
                params.script.len()
            )));
        }

        // The deployment's bounds, on the interpreter's own counters: the
        // script is stopped at `timeout-ms` and a stream is capped at
        // `max-output-bytes` — the interpreter's remaining limits (command and
        // loop counts, its work budget) stay at their defaults, so a script
        // that never ends is stopped by the interpreter even where a busy
        // computation never reaches the deadline.
        let mut builder = Bash::builder().limits(
            ExecutionLimits::new()
                .timeout(Duration::from_millis(timeout_ms))
                .max_stdout_bytes(max_output_bytes)
                .max_stderr_bytes(max_output_bytes),
        );

        // Directory control: when the engine config gives this process a
        // workdir, that directory *is* the interpreter's root — `/` inside the
        // script is the run's own directory, so a relative path resolves inside
        // it and a write lands there rather than anywhere else. `HOME`,
        // `TMPDIR`/`TEMP`/`TMP` and `PWD` point at the root for the tools that
        // default to them, and `ACTS_WORKDIR` names it for the script.
        //
        // Without a workdir there is no host directory to root the interpreter
        // at: it gets bashkit's own in-memory filesystem, so the act still runs
        // and its script still has a filesystem, one with no host behind it.
        if let Some(dir) = ctx.workdir() {
            let fs = RealFs::open(&dir, RealFsMode::ReadWrite)
                .await
                .map_err(|err| {
                    ActError::Package(format!(
                        "failed to open the run's directory {}: {err}",
                        dir.display()
                    ))
                })?;
            builder = builder
                .cwd("/")
                .env("HOME", "/")
                .env("PWD", "/")
                .env("TMPDIR", "/")
                .env("TEMP", "/")
                .env("TMP", "/")
                .env(WORKDIR_ENV, "/")
                .fs(Arc::new(PosixFs::new(fs)));
        }
        let mut bash = builder.build();

        // Cancellation is a third outcome rather than an error: the act did
        // not fail, it was stopped — the action that overrode the task owns
        // the task's state, so a failure of its own would overwrite that
        // decision, and a shutdown would turn every run in flight into an
        // error. The interpreter is dropped with the act, so a script that was
        // waiting stops with it.
        let cancel = ctx.cancellation_token();
        let result = tokio::select! {
            result = bash.exec(&params.script) => result,
            _ = cancel.cancelled() => return Ok(None),
        };
        let result = result.map_err(|err| bound_error(err, timeout_ms))?;

        // The interpreter truncates a stream at the capture limit instead of
        // failing, and a truncated stream is not an act that succeeded with
        // less output: the act fails here, naming the bound the deployment
        // set, which is what a script that floods has to answer for.
        if result.stdout_truncated || result.stderr_truncated {
            return Err(ActError::Package(format!(
                "shell output stream exceeded max-output-bytes limit ({max_output_bytes})"
            )));
        }

        if !result.is_success() {
            let err = String::from_utf8(result.stderr.into_bytes())?;
            return Err(ActError::Package(err));
        }
        let data = String::from_utf8(result.stdout.into_bytes())?;
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

/// What a script's failure means to this package.
///
/// The interpreter reports the deadline as one of its resource limits; the act
/// names the bound it was actually run under instead, which is what the
/// deployment wrote and what an operator can act on. Every other failure — a
/// parse error, a limit of the interpreter's own — is reported as the
/// interpreter states it.
fn bound_error(err: BashError, timeout_ms: u64) -> ActError {
    match err {
        BashError::ResourceLimit(LimitExceeded::Timeout(_)) => timed_out(timeout_ms),
        err => ActError::Package(err.to_string()),
    }
}

/// The act's deadline ran out.
fn timed_out(timeout_ms: u64) -> ActError {
    ActError::Package(format!(
        "shell command timed out after {timeout_ms} ms (timeout-ms)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_policy(allow: &[&str], deny: &[&str]) -> ScriptPolicy {
        ScriptPolicy::new(&ShellConfig {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
        .expect("compile policy")
    }

    /// A script that names another shell is refused where its params are read:
    /// the package runs bash, and says so instead of running the script anyway.
    #[test]
    fn another_shell_is_refused_at_the_params() {
        for shell in ["sh", "nu", "powershell"] {
            let params = serde_json::json!({ "shell": shell, "script": "echo hi" });
            assert!(
                serde_json::from_value::<ShellPackageParams>(params).is_err(),
                "{shell} must not deserialize"
            );
        }

        let params = serde_json::json!({ "shell": "bash", "script": "echo hi" });
        assert!(
            serde_json::from_value::<ShellPackageParams>(params).is_ok(),
            "bash is the shell the package runs"
        );

        // The shell is optional, and its absence means bash too.
        let params = serde_json::json!({ "script": "echo hi" });
        assert!(serde_json::from_value::<ShellPackageParams>(params).is_ok());
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
