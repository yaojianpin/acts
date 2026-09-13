//! Access control for engine operations.
//!
//! Two questions are answered here, and they are deliberately separate:
//!
//! - **Who may run an operation** — a request carries a token; the token
//!   selects a *role*; the role's `allow`/`deny` action patterns decide. A
//!   `deny` always wins, and when the `[acl]` section exists the default is
//!   *deny*: a request without a token (or with an unknown one) resolves to
//!   [`AclConfig::default_role`], or to an empty principal when unset.
//! - **Whose snapshot data a process may read** — the data plane
//!   ([`crate::snapshot`]) is addressed by `target` × `scope`. A role may
//!   declare, per target, which scope patterns it owns; `$subject` is
//!   substituted with the authenticated subject, so `["$subject"]` means
//!   "only my own scope". A role with `allow = ["*"]` is unrestricted and
//!   passes every scope check too.
//!
//! This module holds *policy only*: transport plugins authenticate a request
//! into a [`Principal`] and hand it to
//! [`actions::apply_as`](crate::actions::apply_as), which enforces the
//! action check. Snapshot scope ownership is enforced at two points: on the
//! `snap:*` operations themselves (feed/query APIs), and at seal time — the
//! scheduler re-checks the *process owner* before freezing a snapshot value
//! into a task, so a workflow cannot reach another subject's data by reading
//! `secrets.TOKEN` from a process started with someone else's `uid`.
//!
//! Tokens are never stored in clear text: config entries are either
//! `sha256:<64 hex digits>` or a plaintext token, which is hashed at load.
//!
//! ```toml
//! [acl]
//! enabled = true                     # the section itself already means on
//!
//! # shorthand — this single token is unrestricted (admin)
//! token = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
//!
//! [[acl.role]]
//! name = "operator"
//! tokens = ["sha256:2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"]
//! allow = ["model:ls", "model:get", "proc:*", "snap:get", "snap:ls"]
//! deny = ["model:rm"]
//! snapshot = { secrets = ["$subject"], profile = ["$subject/*"] }
//! ```

use crate::{ActError, Result};
use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Action name that reports the caller's own identity and effective policy.
/// It is implicitly allowed — but only for an already authenticated caller,
/// so it can be used as a startup check without opening a hole.
pub const ACTION_WHOAMI: &str = "acl:whoami";

/// The `[acl]` config section. Only read when the section exists — see
/// [`Acl::from_config`], which turns its presence into "enabled by default".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AclConfig {
    /// Explicit override. `None` (the field absent) means enabled, because a
    /// present `[acl]` section is itself the opt-in.
    pub enabled: Option<bool>,
    /// Shorthand single token granting unrestricted access.
    pub token: Option<String>,
    /// Role applied to a request whose token is absent or unknown. Must name
    /// a configured role.
    pub default_role: Option<String>,
    /// Filesystem root for the processes this policy starts: each one runs in
    /// `<workdir>/<pid>`. Omitted means no directory control, and a process
    /// may touch whatever the server's own account can.
    pub workdir: Option<String>,
    /// `[[acl.role]]` entries.
    pub role: Vec<RoleConfig>,
}

/// One `[[acl.role]]` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct RoleConfig {
    pub name: String,
    /// Accepted tokens: `sha256:<hex>` or plaintext (hashed at load).
    #[serde(default)]
    pub tokens: Vec<String>,
    /// Action patterns the role may run (`*` matches everything).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Action patterns the role must never run; wins over `allow`.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Per snapshot target: the scope patterns the role owns. `$subject` is
    /// replaced by the authenticated subject. A target that is not listed is
    /// not readable. Only consulted when the role is not unrestricted.
    #[serde(default)]
    pub snapshot: HashMap<String, Vec<String>>,
    /// Filesystem root for this role's processes, overriding the `[acl]`
    /// `workdir`. See [`AclConfig::workdir`].
    #[serde(default)]
    pub workdir: Option<String>,
}

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclError {
    /// No token, or a token that matches no role.
    Unauthenticated(String),
    /// An authenticated caller without the right to run the operation.
    Denied(String),
}

impl fmt::Display for AclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AclError::Unauthenticated(msg) => write!(f, "unauthenticated: {msg}"),
            AclError::Denied(msg) => write!(f, "permission denied: {msg}"),
        }
    }
}

impl std::error::Error for AclError {}

impl From<AclError> for ActError {
    fn from(err: AclError) -> Self {
        match err {
            AclError::Unauthenticated(msg) => ActError::Unauthenticated(msg),
            AclError::Denied(msg) => ActError::Denied(msg),
        }
    }
}

/// The scope authority carried by a process: which snapshot targets the
/// process *owner* may read, and under which scope. It is sealed into the
/// process env at start (under [`crate::utils::consts::PROC_OWNER`]) and
/// re-checked by the scheduler at every seal, so a model deployed by anyone
/// cannot widen its own reading scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopePolicy {
    /// Subject the policy belongs to; substituted into `$subject` patterns.
    #[serde(default)]
    pub subject: String,
    /// Unrestricted: every target and scope passes.
    #[serde(default)]
    pub all: bool,
    /// target -> raw scope patterns (`$subject` allowed).
    #[serde(default)]
    pub scopes: HashMap<String, Vec<String>>,
}

impl Default for ScopePolicy {
    /// Unrestricted — the policy of a process that was not started through an
    /// authenticated operation (in-process embedders, engine-internal starts,
    /// and process rows written before this field existed).
    fn default() -> Self {
        Self::unrestricted()
    }
}

impl ScopePolicy {
    pub fn unrestricted() -> Self {
        Self {
            subject: String::new(),
            all: true,
            scopes: HashMap::new(),
        }
    }

    /// Nothing readable — the policy of the anonymous principal under an
    /// enabled ACL.
    pub fn deny_all() -> Self {
        Self {
            subject: String::new(),
            all: false,
            scopes: HashMap::new(),
        }
    }

    /// Whether `scope` of `target` is within this policy.
    pub fn allows(&self, target: &str, scope: &str) -> bool {
        if self.all {
            return true;
        }
        match self.scopes.get(target) {
            Some(patterns) => patterns
                .iter()
                .any(|pattern| scope_matches(&substitute(pattern, &self.subject), scope)),
            None => false,
        }
    }
}

/// An authenticated caller: identity plus the policy compiled from its role.
#[derive(Debug, Clone)]
pub struct Principal {
    subject: String,
    roles: Vec<String>,
    /// Whether a token resolved to this principal. An unauthenticated
    /// principal is refused as such, not as an authorization failure, so a
    /// transport can answer "no credential" distinctly from "not allowed".
    authenticated: bool,
    all: bool,
    allow: Vec<GlobMatcher>,
    deny: Vec<GlobMatcher>,
    allow_pat: Vec<String>,
    deny_pat: Vec<String>,
    scopes: HashMap<String, Vec<String>>,
    /// Filesystem root this principal's processes run under; `None` when the
    /// policy declares none (no directory control).
    workdir: Option<PathBuf>,
}

impl Principal {
    /// A principal that passes every check — the identity used when no ACL is
    /// configured at all.
    pub fn unrestricted() -> Self {
        Self {
            subject: "system".to_string(),
            roles: Vec::new(),
            authenticated: true,
            all: true,
            allow: Vec::new(),
            deny: Vec::new(),
            allow_pat: vec!["*".to_string()],
            deny_pat: Vec::new(),
            scopes: HashMap::new(),
            workdir: None,
        }
    }

    /// The anonymous caller under an enabled ACL: no token matched, and no
    /// `default_role` was configured — every operation is refused.
    pub fn anonymous() -> Self {
        Self {
            subject: "anonymous".to_string(),
            roles: Vec::new(),
            authenticated: false,
            all: false,
            allow: Vec::new(),
            deny: Vec::new(),
            allow_pat: Vec::new(),
            deny_pat: Vec::new(),
            scopes: HashMap::new(),
            workdir: None,
        }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// Whether this principal is unrestricted (admin role, shorthand token,
    /// or a disabled ACL).
    pub fn is_unrestricted(&self) -> bool {
        self.all
    }

    /// Whether a token resolved to this principal.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    pub fn check(&self, action: &str) -> std::result::Result<(), AclError> {
        if !self.authenticated {
            return Err(AclError::Unauthenticated(
                "a valid acl token is required; see [acl] in the config".to_string(),
            ));
        }
        if self.deny.iter().any(|glob| glob.is_match(action)) {
            return Err(AclError::Denied(format!(
                "{} denies action '{action}'",
                self.describe()
            )));
        }
        if self.all || self.allow.iter().any(|glob| glob.is_match(action)) {
            return Ok(());
        }
        Err(AclError::Denied(format!(
            "action '{action}' is not allowed for {}",
            self.describe()
        )))
    }

    pub fn check_scope(&self, target: &str, scope: &str) -> std::result::Result<(), AclError> {
        if !self.authenticated {
            return Err(AclError::Unauthenticated(
                "a valid acl token is required; see [acl] in the config".to_string(),
            ));
        }
        if self.scope_policy().allows(target, scope) {
            return Ok(());
        }
        Err(AclError::Denied(format!(
            "scope '{scope}' of snapshot '{target}' is not owned by subject '{}'",
            self.subject
        )))
    }
    /// The filesystem root this principal's processes run under, or `None`
    /// when the policy declares none.
    pub fn workdir(&self) -> Option<&Path> {
        self.workdir.as_deref()
    }

    /// The scope authority to seal into a process started by this principal.
    pub fn scope_policy(&self) -> ScopePolicy {
        ScopePolicy {
            subject: self.subject.clone(),
            all: self.all,
            scopes: self.scopes.clone(),
        }
    }

    /// What `acl:whoami` answers: the identity and the patterns in force.
    pub fn to_value(&self) -> JsonValue {
        json!({
            "subject": self.subject,
            "roles": self.roles,
            "unrestricted": self.all,
            "allow": self.allow_pat,
            "deny": self.deny_pat,
            "scopes": self.scopes,
            "workdir": self.workdir.as_ref().map(|dir| dir.display().to_string()),
        })
    }

    fn describe(&self) -> String {
        if self.roles.is_empty() {
            format!("subject '{}'", self.subject)
        } else {
            format!("role(s) {}", self.roles.join(", "))
        }
    }
}

/// One role, with its patterns compiled.
#[derive(Debug, Clone)]
struct CompiledRole {
    name: String,
    all: bool,
    allow: Vec<GlobMatcher>,
    deny: Vec<GlobMatcher>,
    allow_pat: Vec<String>,
    deny_pat: Vec<String>,
    scopes: HashMap<String, Vec<String>>,
    workdir: Option<PathBuf>,
}

/// The compiled ACL: a token index plus the roles it resolves to.
#[derive(Debug, Clone)]
pub struct Acl {
    enabled: bool,
    roles: Vec<CompiledRole>,
    /// sha256 hex of an accepted token -> role index.
    index: HashMap<String, usize>,
    /// Role applied to an absent/unknown token (`default_role`).
    default_role: Option<usize>,
    /// `[acl] workdir` — the root every role without its own runs under.
    workdir: Option<PathBuf>,
}

impl Default for Acl {
    /// A disabled ACL: every operation passes with
    /// [`Principal::unrestricted`].
    fn default() -> Self {
        Self {
            enabled: false,
            roles: Vec::new(),
            index: HashMap::new(),
            default_role: None,
            workdir: None,
        }
    }
}

impl Acl {
    /// A disabled ACL — no enforcement.
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Compile `[acl]`. Every malformed entry is an error: a policy that
    /// cannot be enforced must fail startup instead of silently allowing
    /// (or silently refusing) traffic.
    pub fn from_config(config: &AclConfig) -> Result<Self> {
        let enabled = config.enabled.unwrap_or(true);
        if !enabled {
            return Ok(Self::disabled());
        }

        let mut roles: Vec<CompiledRole> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut seen: HashMap<String, String> = HashMap::new();

        // The shorthand token is an extra, unrestricted role.
        if let Some(token) = config.token.as_deref().filter(|t| !t.trim().is_empty()) {
            roles.push(CompiledRole {
                name: "admin".to_string(),
                all: true,
                allow: Vec::new(),
                deny: Vec::new(),
                allow_pat: vec!["*".to_string()],
                deny_pat: Vec::new(),
                scopes: HashMap::new(),
                workdir: None,
            });
            let hash = hash_token(token)?;
            index.insert(hash, 0);
        }

        for role in &config.role {
            let idx = roles.len();
            roles.push(compile_role(role)?);
            for token in &role.tokens {
                if token.trim().is_empty() {
                    continue;
                }
                let hash = hash_token(token)?;
                // A token that selects one of two roles would silently pick
                // one of them; make the ambiguity a config error.
                if let Some(previous) = seen.insert(hash.clone(), role.name.clone()) {
                    return Err(ActError::Config(format!(
                        "acl token is assigned to both role '{previous}' and role '{}'",
                        role.name
                    )));
                }
                index.insert(hash, idx);
            }
        }

        if roles.is_empty() {
            return Err(ActError::Config(
                "acl is enabled but neither a token nor a role is configured".to_string(),
            ));
        }
        if index.is_empty() {
            return Err(ActError::Config(
                "acl is enabled but no role declares any token".to_string(),
            ));
        }

        let default_role = match config.default_role.as_deref() {
            // `deny` spelled out is the same as omitting it.
            None | Some("") | Some("deny") => None,
            Some(name) => Some(roles.iter().position(|role| role.name == name).ok_or_else(
                || {
                    ActError::Config(format!(
                        "acl default_role '{name}' is not a configured role"
                    ))
                },
            )?),
        };

        Ok(Self {
            enabled: true,
            roles,
            index,
            default_role,
            workdir: compile_workdir(config.workdir.as_deref(), "acl")?,
        })
    }

    /// Resolve a request's token into a principal. An unknown or missing
    /// token falls back to `default_role` when one is configured, and is
    /// otherwise refused.
    pub fn authenticate(&self, token: Option<&str>) -> std::result::Result<Principal, AclError> {
        if !self.enabled {
            return Ok(Principal::unrestricted());
        }

        if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) {
            // A malformed-but-present token is never an error here: it simply
            // matches no role, exactly like a wrong one.
            if let Ok(hash) = hash_token(token)
                && let Some(&idx) = self.index.get(&hash)
            {
                return Ok(self.principal(idx));
            }
        }

        match self.default_role {
            Some(idx) => Ok(self.principal(idx)),
            None => Err(AclError::Unauthenticated(
                "a valid acl token is required; see [acl] in the config".to_string(),
            )),
        }
    }

    /// The principal a request without a valid token resolves to (used by
    /// in-process callers that do not authenticate).
    pub fn anonymous(&self) -> Principal {
        if !self.enabled {
            return Principal::unrestricted();
        }
        match self.default_role {
            Some(idx) => self.principal(idx),
            None => Principal::anonymous(),
        }
    }

    fn principal(&self, idx: usize) -> Principal {
        let role = &self.roles[idx];
        Principal {
            // The role name doubles as the subject: tokens are opaque, so
            // there is no better name to attribute a request to.
            subject: role.name.clone(),
            roles: vec![role.name.clone()],
            authenticated: true,
            all: role.all,
            allow: role.allow.clone(),
            deny: role.deny.clone(),
            allow_pat: role.allow_pat.clone(),
            deny_pat: role.deny_pat.clone(),
            scopes: role.scopes.clone(),
            workdir: role.workdir.clone().or_else(|| self.workdir.clone()),
        }
    }
}

fn compile_role(role: &RoleConfig) -> Result<CompiledRole> {
    if role.name.trim().is_empty() {
        return Err(ActError::Config(
            "acl role name cannot be empty".to_string(),
        ));
    }
    let all = role.allow.iter().any(|p| p == "*");
    let allow = compile_patterns(&role.allow, &role.name, "allow")?;
    let deny = compile_patterns(&role.deny, &role.name, "deny")?;
    for (target, patterns) in &role.snapshot {
        if target.trim().is_empty() {
            return Err(ActError::Config(format!(
                "acl role '{}' has a snapshot rule without a target name",
                role.name
            )));
        }
        // Validate the patterns now; a bad one must not degrade into
        // "matches nothing" at seal time.
        compile_patterns(patterns, &role.name, "snapshot")?;
    }

    Ok(CompiledRole {
        name: role.name.clone(),
        all,
        allow,
        deny,
        allow_pat: role.allow.clone(),
        deny_pat: role.deny.clone(),
        scopes: role.snapshot.clone(),
        workdir: compile_workdir(role.workdir.as_deref(), &role.name)?,
    })
}

/// Normalize a configured workdir root. An empty value is a config error
/// rather than "no directory control": the two cannot be told apart in the
/// result, and a typo must not silently drop the confinement.
fn compile_workdir(workdir: Option<&str>, owner: &str) -> Result<Option<PathBuf>> {
    match workdir {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Err(ActError::Config(format!(
            "acl {owner} workdir cannot be empty; remove the key to run without directory control"
        ))),
        Some(value) => Ok(Some(PathBuf::from(value.trim()))),
    }
}

fn compile_patterns(patterns: &[String], role: &str, field: &str) -> Result<Vec<GlobMatcher>> {
    patterns
        .iter()
        .map(|pattern| {
            Glob::new(pattern)
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    ActError::Config(format!(
                        "acl role '{role}' has an invalid {field} pattern '{pattern}': {err}"
                    ))
                })
        })
        .collect()
}

/// Normalize a configured token into its lowercase sha256 hex digest. A
/// `sha256:` prefix means the value is already a digest; anything else is
/// treated as plaintext (the `requirepass`-style form) and hashed.
fn hash_token(token: &str) -> Result<String> {
    let token = token.trim();
    if let Some(hex) = token.strip_prefix("sha256:") {
        let hex = hex.trim().to_ascii_lowercase();
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(hex);
        }
        return Err(ActError::Config(
            "acl token 'sha256:…' must carry exactly 64 hex digits".to_string(),
        ));
    }
    Ok(sha256_hex(token))
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Replace every `$subject` in a pattern with `subject`.
fn substitute(pattern: &str, subject: &str) -> String {
    pattern.replace("$subject", subject)
}

/// Match one scope against one pattern. A pattern without glob metacharacters
/// — the common `["$subject"]` case — is compared directly, so the scheduler
/// seal path does not compile a matcher per task.
fn scope_matches(pattern: &str, scope: &str) -> bool {
    let is_glob = pattern
        .bytes()
        .any(|b| matches!(b, b'*' | b'?' | b'[' | b'{'));
    if !is_glob {
        return pattern == scope;
    }
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher().is_match(scope))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml_text: &str) -> AclConfig {
        toml::from_str::<AclConfig>(toml_text).unwrap()
    }

    fn operator_acl() -> Acl {
        Acl::from_config(&config(
            r#"
            [[role]]
            name = "operator"
            tokens = ["op-secret"]
            allow = ["model:ls", "model:get", "proc:*", "snap:get", "snap:ls"]
            deny = ["proc:start_from_model"]
            snapshot = { secrets = ["$subject"], profile = ["$subject", "$subject/*"] }
            "#,
        ))
        .unwrap()
    }

    #[test]
    fn section_present_means_enabled() {
        let acl = operator_acl();
        assert!(acl.enabled());
    }

    #[test]
    fn explicit_false_disables() {
        let acl = Acl::from_config(&config(r#"enabled = false"#)).unwrap();
        assert!(!acl.enabled());
        assert!(acl.anonymous().is_unrestricted());
        assert!(acl.authenticate(None).unwrap().is_unrestricted());
    }

    #[test]
    fn plaintext_and_hashed_tokens_both_authenticate() {
        let plain = hash_token("op-secret").unwrap();
        // "op-secret" is also a sha256: form fixture below
        assert_eq!(plain.len(), 64);
        assert_eq!(
            hash_token(&format!("sha256:{plain}")).unwrap(),
            hash_token("op-secret").unwrap()
        );

        let acl = operator_acl();
        let principal = acl.authenticate(Some("op-secret")).unwrap();
        assert_eq!(principal.subject(), "operator");
        assert!(acl.authenticate(Some("wrong")).is_err());
        assert!(acl.authenticate(None).is_err());
    }

    #[test]
    fn unknown_token_never_falls_back_to_a_role() {
        let acl = operator_acl();
        let err = acl.authenticate(Some("nope")).unwrap_err();
        assert!(matches!(err, AclError::Unauthenticated(_)));
    }

    #[test]
    fn default_role_applies_to_absent_and_unknown_tokens() {
        let acl = Acl::from_config(&config(
            r#"
            default_role = "guest"
            [[role]]
            name = "guest"
            tokens = ["guest-token"]
            allow = ["model:ls"]
            "#,
        ))
        .unwrap();

        let absent = acl.authenticate(None).unwrap();
        assert_eq!(absent.subject(), "guest");
        assert_eq!(
            acl.authenticate(Some("unknown")).unwrap().subject(),
            "guest"
        );
        assert!(absent.check("model:ls").is_ok());
        assert!(absent.check("model:deploy").is_err());
    }

    #[test]
    fn deny_wins_over_allow() {
        let acl = operator_acl();
        let principal = acl.authenticate(Some("op-secret")).unwrap();

        // `proc:*` allows it, the explicit deny refuses it.
        assert!(principal.check("proc:start").is_ok());
        assert!(matches!(
            principal.check("proc:start_from_model"),
            Err(AclError::Denied(_))
        ));
        assert!(principal.check("act:complete").is_err());
    }

    #[test]
    fn shorthand_token_is_unrestricted() {
        let acl = Acl::from_config(&config(r#"token = "root-secret""#)).unwrap();
        let principal = acl.authenticate(Some("root-secret")).unwrap();
        assert!(principal.is_unrestricted());
        assert!(principal.check("model:rm").is_ok());
        assert!(principal.check_scope("secrets", "anyone").is_ok());
    }

    #[test]
    fn wildcard_allow_grants_every_scope() {
        let acl = Acl::from_config(&config(
            r#"
            [[role]]
            name = "root"
            tokens = ["root-secret"]
            allow = ["*"]
            "#,
        ))
        .unwrap();
        let principal = acl.authenticate(Some("root-secret")).unwrap();
        assert!(principal.is_unrestricted());
        assert!(principal.check("msg:clear").is_ok());
        assert!(principal.check_scope("secrets", "other").is_ok());
    }

    #[test]
    fn scope_ownership_follows_the_subject() {
        let acl = operator_acl();
        let principal = acl.authenticate(Some("op-secret")).unwrap();

        assert!(principal.check_scope("secrets", "operator").is_ok());
        assert!(principal.check_scope("profile", "operator").is_ok());
        assert!(principal.check_scope("profile", "operator/proj-a").is_ok());
        assert!(principal.check_scope("secrets", "someone-else").is_err());
        assert!(principal.check_scope("profile", "someone-else").is_err());
        // a target with no rule is not readable
        assert!(principal.check_scope("audit", "operator").is_err());
    }

    #[test]
    fn scope_substitution_reads_the_owner_subject() {
        let policy = ScopePolicy {
            subject: "u1".to_string(),
            all: false,
            scopes: HashMap::from([("secrets".to_string(), vec!["$subject".to_string()])]),
        };
        assert!(policy.allows("secrets", "u1"));
        assert!(!policy.allows("secrets", "u2"));
    }

    #[test]
    fn hashing_a_malformed_digest_is_a_config_error() {
        let err = Acl::from_config(&config(
            r#"
            [[role]]
            name = "r"
            tokens = ["sha256:nothex"]
            allow = ["*"]
            "#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("64 hex digits"), "{err}");
    }

    #[test]
    fn an_invalid_action_pattern_fails_startup() {
        let err = Acl::from_config(&config(
            r#"
            [[role]]
            name = "r"
            tokens = ["t"]
            allow = ["act:{unclosed"]
            "#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("invalid allow pattern"), "{err}");
    }
    #[test]
    fn an_enabled_acl_without_any_token_is_a_config_error() {
        let err = Acl::from_config(&config(r#"enabled = true"#)).unwrap_err();
        assert!(
            err.to_string().contains("neither a token nor a role"),
            "{err}"
        );
    }

    #[test]
    fn a_role_without_tokens_is_a_config_error() {
        let err = Acl::from_config(&config(
            r#"
            [[role]]
            name = "r"
            allow = ["*"]
            "#,
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("no role declares any token"),
            "{err}"
        );
    }

    #[test]
    fn an_empty_role_name_is_a_config_error() {
        let err = Acl::from_config(&config(
            r#"
            [[role]]
            name = "  "
            tokens = ["t"]
            allow = ["*"]
            "#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("name cannot be empty"), "{err}");
    }

    #[test]
    fn workdir_defaults_to_none_and_role_overrides_the_section() {
        // No workdir anywhere: no directory control.
        let acl = operator_acl();
        assert!(
            acl.authenticate(Some("op-secret"))
                .unwrap()
                .workdir()
                .is_none()
        );

        // Section-level root applies to every role...
        let acl = Acl::from_config(&config(
            r#"
            workdir = "/srv/acts"
            [[role]]
            name = "r"
            tokens = ["t"]
            allow = ["*"]
            "#,
        ))
        .unwrap();
        assert_eq!(
            acl.authenticate(Some("t")).unwrap().workdir().unwrap(),
            Path::new("/srv/acts")
        );

        // ...and a role may point somewhere else.
        let acl = Acl::from_config(&config(
            r#"
            workdir = "/srv/acts"
            [[role]]
            name = "a"
            tokens = ["ta"]
            allow = ["*"]
            [[role]]
            name = "b"
            tokens = ["tb"]
            allow = ["*"]
            workdir = "/srv/tenant-b"
            "#,
        ))
        .unwrap();
        assert_eq!(
            acl.authenticate(Some("ta")).unwrap().workdir().unwrap(),
            Path::new("/srv/acts")
        );
        assert_eq!(
            acl.authenticate(Some("tb")).unwrap().workdir().unwrap(),
            Path::new("/srv/tenant-b")
        );
    }

    #[test]
    fn an_empty_workdir_is_a_config_error() {
        // An empty value cannot be told from "no control", so a typo must not
        // silently drop the confinement.
        for text in [
            r#"
            workdir = "  "
            [[role]]
            name = "r"
            tokens = ["t"]
            allow = ["*"]
            "#,
            r#"
            [[role]]
            name = "r"
            tokens = ["t"]
            allow = ["*"]
            workdir = ""
            "#,
        ] {
            let err = Acl::from_config(&config(text)).unwrap_err();
            assert!(err.to_string().contains("workdir cannot be empty"), "{err}");
        }
    }

    #[test]
    fn whoami_reports_the_workdir_without_leaking_tokens() {
        let acl = Acl::from_config(&config(
            r#"
            workdir = "/srv/acts"
            [[role]]
            name = "r"
            tokens = ["top-secret"]
            allow = ["*"]
            "#,
        ))
        .unwrap();
        let value = acl.authenticate(Some("top-secret")).unwrap().to_value();
        assert_eq!(value["workdir"], "/srv/acts");
        assert!(!value.to_string().contains("top-secret"));
    }

    #[test]
    fn a_snapshot_rule_without_a_target_is_a_config_error() {
        let err = Acl::from_config(&config(
            r#"
            [[role]]
            name = "r"
            tokens = ["t"]
            allow = ["*"]
            snapshot = { "  " = ["$subject"] }
            "#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("without a target name"), "{err}");
    }

    #[test]
    fn one_token_cannot_select_two_roles() {
        let err = Acl::from_config(&config(
            r#"
            [[role]]
            name = "a"
            tokens = ["shared"]
            allow = ["*"]
            [[role]]
            name = "b"
            tokens = ["shared"]
            allow = ["*"]
            "#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("assigned to both"), "{err}");
    }

    #[test]
    fn an_unknown_default_role_is_a_config_error() {
        let err = Acl::from_config(&config(
            r#"
            default_role = "ghost"
            [[role]]
            name = "r"
            tokens = ["t"]
            allow = ["*"]
            "#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("not a configured role"), "{err}");
    }

    #[test]
    fn whoami_reports_the_effective_policy() {
        let acl = operator_acl();
        let value = acl.authenticate(Some("op-secret")).unwrap().to_value();
        assert_eq!(value["subject"], "operator");
        assert_eq!(value["unrestricted"], false);
        assert_eq!(value["deny"][0], "proc:start_from_model");
        assert_eq!(value["scopes"]["secrets"][0], "$subject");
        // and it never leaks the token or its digest
        let text = value.to_string();
        assert!(!text.contains("op-secret"));
        assert!(!text.contains(&hash_token("op-secret").unwrap()));
    }

    #[test]
    fn anonymous_under_an_enabled_acl_is_denied_everything() {
        let acl = operator_acl();
        let anon = acl.anonymous();
        assert!(anon.check("model:ls").is_err());
        assert!(anon.check_scope("profile", "").is_err());
    }
}
