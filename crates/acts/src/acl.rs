//! Access control: the policy types the engine enforces, and the port its
//! implementation plugs into.
//!
//! Two questions are answered here, and they are deliberately separate:
//!
//! - **Who may run an operation** — a caller logs in (`acl:login`) with a
//!   *user* and a *password* and receives a session token; the token selects
//!   the user; the user's `allow`/`deny` patterns decide. A pattern is either
//!   a *command* glob (`model:*`, `proc:start`) or a *catalog* reference
//!   (`@read`, `@write`, `@deploy`, `@execute`, `@all`) naming the group an
//!   action belongs to — the Redis ACL shape of `+command` and `+@category`.
//!   A `deny` always wins, and the default is *deny*: a request without a
//!   token (or with an unknown or expired one) resolves to the read-only
//!   [`ANONYMOUS_ROLE`] principal.
//! - **Whose data a process may touch** — a user owns the snapshot scopes it
//!   may read (per target, `$subject` allowed) and the *resources*
//!   (`patterns`) it may deploy and run: a workflow declares its resource
//!   name as `rn` (`orders:eu`), and a user may only deploy and start
//!   workflows whose `rn` matches one of its patterns — the Redis
//!   key-pattern shape.
//!
//! This module holds *policy only*: the users and live sessions — their
//! storage, passwords, login and expiry — are the [`AccessControl`]
//! implementation's business, and the shipped one lives in the `acts-acl`
//! crate (`acts_acl::UserAcl`), which keeps its rows in the engine's store.
//! An engine without one runs the anonymous policy below.
//!
//! Transport plugins authenticate a request into a [`Principal`] and hand it
//! to [`actions::apply_as`](crate::actions::apply_as), which enforces the
//! action check. Snapshot scope ownership is enforced at two points: on the
//! `snap:*` operations themselves (feed/query APIs), and at seal time — the
//! scheduler re-checks the *process owner* before freezing a snapshot value
//! into a task, so a workflow cannot reach another subject's data by reading
//! `secrets.TOKEN` from a process started with someone else's `uid`.

use crate::store::Store;
use crate::{ActError, Result};
use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Action name that reports the caller's own identity and effective policy.
/// It is implicitly allowed — but only for an already authenticated caller,
/// so it can be used as a startup check without opening a hole.
pub const ACTION_WHOAMI: &str = "acl:whoami";

/// Action name a subscription is checked against. A stream of workflow
/// messages is a read of the message face, so opening one is an operation like
/// any other: a user may subscribe only if `msg:sub` is in its `allow` list.
pub const ACTION_SUBSCRIBE: &str = "msg:sub";

/// `acl:login` — exchange user + password for a session token pair. Allowed
/// without a credential: the credentials are the payload.
pub const ACTION_LOGIN: &str = "acl:login";
/// `acl:refresh` — exchange a refresh token for a rotated token pair. Allowed
/// without a credential: the refresh token is the credential.
pub const ACTION_REFRESH: &str = "acl:refresh";
/// `acl:logout` — drop the session the request's own token belongs to.
/// Implicitly allowed for an authenticated caller (it revokes the caller's
/// own credential, nobody else's).
pub const ACTION_LOGOUT: &str = "acl:logout";
/// `acl:setuser` — create or update one user (passwords, command/catalog
/// patterns, resource patterns, snapshot scopes).
pub const ACTION_SETUSER: &str = "acl:setuser";
/// `acl:deluser` — delete one user (never the builtin admin).
pub const ACTION_DELUSER: &str = "acl:deluser";
/// `acl:getuser` — read one user's effective policy.
pub const ACTION_GETUSER: &str = "acl:getuser";
/// `acl:users` — list user names.
pub const ACTION_USERS: &str = "acl:users";

/// The role an unauthenticated caller runs under, and the subject it is
/// attributed to.
pub const ANONYMOUS_ROLE: &str = "anonymous";

/// What [`ANONYMOUS_ROLE`] may do: the catalogue reads, and nothing else.
///
/// An engine answers to anyone, so it must answer to the least anyone could
/// be trusted with. That least is the *catalogue*: which models are deployed
/// and which packages exist. Everything else is out — writes, control
/// actions, admin actions, snapshot data, subscriptions. Log in to get more.
pub const ANONYMOUS_ALLOW: &[&str] = &["model:ls", "model:get", "pack:get", "pack:ls"];

/// Every catalog group a user's `allow`/`deny` list may name, plus the
/// catch-all. A token that matches none of them is a config error rather
/// than a grant (or a denial) that silently does nothing.
pub const CATALOG_GROUPS: &[&str] = &["read", "write", "deploy", "execute", "all"];

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclError {
    /// No token, an unknown one, or a bad user/password.
    Unauthenticated(String),
    /// An authenticated caller without the right to run the operation.
    Denied(String),
}

impl fmt::Display for AclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AclError::Unauthenticated(msg) => write!(f, "unauthenticated: {msg}"),
            AclError::Denied(msg) => write!(f, "denied: {msg}"),
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

impl From<ActError> for AclError {
    fn from(err: ActError) -> Self {
        match err {
            ActError::Unauthenticated(msg) => AclError::Unauthenticated(msg),
            ActError::Denied(msg) => AclError::Denied(msg),
            other => AclError::Denied(other.to_string()),
        }
    }
}

/// One user's grants, as the engine's policy language sees them: the plain
/// data an [`AccessControl`] implementation stores and hands back, before it
/// is compiled into a [`Principal`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserPolicy {
    /// user name; doubles as the subject a request is attributed to
    pub name: String,
    /// a disabled user authenticates nothing, `acl:login` included
    pub enabled: bool,
    /// command/catalog patterns the user may run; `*`/`@all` = unrestricted
    pub allow: Vec<String>,
    /// command/catalog patterns the user must never run; wins over `allow`
    pub deny: Vec<String>,
    /// resource-name (`rn`) patterns the user may deploy and run
    pub patterns: Vec<String>,
    /// per snapshot target, the scope patterns the user owns
    /// (`$subject` is replaced by the user name)
    pub snapshot: HashMap<String, Vec<String>>,
}

/// One `acl:setuser` request. `None` fields leave the user's setting
/// unchanged (or take the default, on create); list fields replace; password
/// entries are added and removed by value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserSpec {
    /// User name. Required; it is the subject requests are attributed to and
    /// it prefixes every subscription key the user opens.
    pub name: String,
    /// `Some(false)` disables the user (login refused, sessions dead);
    /// `None` keeps the current value (new users default to enabled).
    pub enabled: Option<bool>,
    /// Passwords to add (plaintext; hashed by the implementation).
    #[serde(default)]
    pub add_passwords: Vec<String>,
    /// Passwords to remove, by their plaintext value.
    #[serde(default)]
    pub rm_passwords: Vec<String>,
    /// Command/catalog patterns the user may run (`model:*`, `@read`,
    /// `@deploy`, `@execute`); replaces the current list.
    pub allow: Option<Vec<String>>,
    /// Command/catalog patterns the user must never run; wins over `allow`.
    /// Replaces the current list.
    pub deny: Option<Vec<String>>,
    /// Resource (`rn`) patterns; replaces the current list.
    pub patterns: Option<Vec<String>>,
    /// Per snapshot target, the scope patterns the user owns; replaces the
    /// current map. `$subject` is allowed.
    pub snapshot: Option<HashMap<String, Vec<String>>>,
}

/// What `acl:login` and `acl:refresh` answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginTokens {
    /// The access token: present it as the `authorization: Bearer …`
    /// credential on every request.
    pub token: String,
    /// The refresh token: present it to `acl:refresh` when the access token
    /// expired. Every refresh rotates the pair.
    pub refresh_token: String,
    /// Access-token lifetime, seconds.
    pub expires_in: i64,
    /// Refresh-token lifetime, seconds.
    pub refresh_expires_in: i64,
}

/// The authority carried by a process: which snapshot targets the process
/// *owner* may read and under which scope, which workflow resources (`rn`)
/// it may deploy and run, plus the directory root its filesystem access is
/// confined to. It is sealed into the process env at start (under
/// [`crate::utils::consts::PROC_OWNER`]) and re-checked by the scheduler at
/// every seal, so a model deployed by anyone cannot widen its own reading
/// scope — and, because the root travels here rather than as a start option,
/// a caller cannot place a run outside the directory its policy names
/// either.
///
/// A policy comes from one place: the principal that started the run, through
/// [`Principal::scope_policy`]. Nothing widens a run's reading by omission —
/// see [`ScopePolicy::default`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopePolicy {
    /// Subject the policy belongs to; substituted into `$subject` patterns.
    #[serde(default)]
    pub subject: String,
    /// Unrestricted: every target, scope and resource passes.
    #[serde(default)]
    pub all: bool,
    /// target -> raw scope patterns (`$subject` allowed).
    #[serde(default)]
    pub scopes: HashMap<String, Vec<String>>,
    /// Raw resource-name (`rn`) patterns. A model's `rn` must match one of
    /// them — an empty `rn` only passes when `all`.
    #[serde(default)]
    pub rn: Vec<String>,
    /// Whether the `rn` grant above decides anything at all. A policy that
    /// came from a caller (a [`Principal`]) enforces it; the policy a run
    /// carries when nobody sealed one — an engine-internal start, a schedule
    /// trigger, a subflow inheriting a parent that had no caller either —
    /// does not, because there is no user whose resources it could name. The
    /// same asymmetry as the snapshot scopes, read the other way: an absent
    /// *caller* cannot restrict resources without inventing a user, while an
    /// absent *scope grant* still reads nothing. Absent in older process rows,
    /// which is why it defaults to false.
    #[serde(default)]
    pub enforce_rn: bool,
    /// Filesystem root this process runs under: its own directory is
    /// `<workdir_root>/<pid>`, created at start (that directory — not this
    /// root — is what `Process::workdir` and `Context::workdir` answer, and
    /// what `$env.WORK_DIR` names). Compiled from the engine config (see
    /// [`crate::Config::workdir`]); `None` means no directory control.
    #[serde(default)]
    pub workdir_root: Option<std::path::PathBuf>,
}

impl Default for ScopePolicy {
    /// Nothing readable — what a policy with no grants is. A run carries this
    /// when no caller authority was sealed into it: an engine-internal start
    /// (a schedule trigger, or an in-process embedder calling
    /// `Runtime::start`), and process rows written before this field existed.
    ///
    /// An absent authority is not an unlimited one. The three alternative
    /// readings are all wrong: *unrestricted* hands every run the whole data
    /// plane, *inherit the model's deployer* is not something a start can
    /// know, and *fail the start* would refuse runs that read no snapshot at
    /// all. So the run starts and reads nothing — it may still execute every
    /// task that needs no owned data, and a task that does need it fails at
    /// its seal with the subject it lacks, which is a readable error rather
    /// than a silent grant. Only a policy that came from a caller is ever
    /// unrestricted: a disabled ACL resolves every caller to
    /// [`Principal::unrestricted`], and that principal's
    /// [`Principal::scope_policy`] is where one comes from.
    fn default() -> Self {
        Self::deny_all()
    }
}

impl ScopePolicy {
    /// Every target, scope and resource passes. Only a policy compiled from a
    /// principal is ever this — the anonymous role is not, and neither is
    /// [`ScopePolicy::default`].
    pub fn unrestricted() -> Self {
        Self {
            subject: String::new(),
            all: true,
            scopes: HashMap::new(),
            rn: Vec::new(),
            enforce_rn: true,
            workdir_root: None,
        }
    }

    /// Nothing readable, and no workdir. The policy of the anonymous principal
    /// under an enabled ACL, and the default of a policy nobody sealed.
    pub fn deny_all() -> Self {
        Self {
            subject: String::new(),
            all: false,
            scopes: HashMap::new(),
            rn: Vec::new(),
            enforce_rn: false,
            workdir_root: None,
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

    /// Whether the resource name `rn` a workflow declares is within this
    /// policy. A workflow without an `rn` claims no resource, so only an
    /// unrestricted policy may deploy or start it. A policy nobody compiled
    /// from a caller does not decide this at all — see `enforce_rn`.
    pub fn allows_rn(&self, rn: &str) -> bool {
        if !self.enforce_rn || self.all {
            return true;
        }
        if rn.is_empty() {
            return false;
        }
        self.rn
            .iter()
            .any(|pattern| scope_matches(&substitute(pattern, &self.subject), rn))
    }
}

/// The catalog (command group) an action belongs to — the `@category` of a
/// user's `allow`/`deny` list. Four groups cover the action set, plus `all`:
///
/// - `read` — the catalogue and row reads, `acl:whoami`/`acl:getuser`/
///   `acl:users`, and `msg:sub` (a subscription is a read of the stream)
/// - `deploy` — putting work into the catalogues: `model:deploy`,
///   `pack:publish`
/// - `execute` — running and driving that work: `proc:start`,
///   `proc:start_from_model`, `evt:start`, every `act:*`, and `msg:ack`
/// - `write` — changing or destroying stored state: every `*:rm`,
///   `msg:redo`/`msg:clear`, the snapshot feeds, and the user management
/// - `all` — every action (`@all` in a list is the unrestricted grant, like
///   `*`)
///
/// An action not named here is `write`: an unknown operation is never a read.
pub fn action_catalog(action: &str) -> &'static str {
    match action {
        "model:ls" | "model:get" | "pack:ls" | "pack:get" | "proc:ls" | "proc:get" | "task:ls"
        | "task:get" | "msg:ls" | "msg:get" | "evt:ls" | "evt:get" | "snap:get" | "snap:ls"
        | ACTION_WHOAMI | ACTION_GETUSER | ACTION_USERS | ACTION_SUBSCRIBE => "read",
        "model:deploy" | "pack:publish" => "deploy",
        "proc:start"
        | "proc:start_from_model"
        | "evt:start"
        | "act:push"
        | "act:remove"
        | "act:submit"
        | "act:complete"
        | "act:abort"
        | "act:cancel"
        | "act:back"
        | "act:skip"
        | "act:error"
        | "msg:ack" => "execute",
        _ => "write",
    }
}

/// A token that grants (or denies) everything: `@all`, `@*` or `*`.
fn is_all_token(token: &str) -> bool {
    matches!(token.trim(), "*" | "@*" | "@all")
}

/// Split one `allow`/`deny` list into its two forms: compiled command globs,
/// and catalog tokens (`@read`, `@deploy`, `@all`) kept as patterns over the
/// group name.
fn split_tokens(
    tokens: &[String],
    user: &str,
    field: &str,
) -> Result<(Vec<GlobMatcher>, Vec<String>)> {
    let mut commands = Vec::new();
    let mut catalogs = Vec::new();
    for token in tokens {
        let token = token.trim();
        if let Some(pattern) = token.strip_prefix('@') {
            if pattern.is_empty() {
                return Err(ActError::Config(format!(
                    "acl user '{user}' has an empty {field} catalog token"
                )));
            }
            // a token that names no group would grant (or deny) nothing at
            // all — the one failure mode a typo must not have
            if !CATALOG_GROUPS
                .iter()
                .any(|group| scope_matches(pattern, group))
            {
                return Err(ActError::Config(format!(
                    "acl user '{user}' has a {field} catalog token '@{pattern}' that names no \
                     catalog group (read, write, deploy, execute, all)"
                )));
            }
            catalogs.push(pattern.to_string());
        } else {
            let glob = Glob::new(token)
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    ActError::Config(format!(
                        "acl user '{user}' has an invalid {field} pattern '{token}': {err}"
                    ))
                })?;
            commands.push(glob);
        }
    }
    Ok((commands, catalogs))
}

/// An authenticated caller: identity plus the policy compiled from its user.
#[derive(Debug, Clone)]
pub struct Principal {
    subject: String,
    /// Whether a token resolved to this principal. An unauthenticated
    /// principal is refused as such, not as an authorization failure, so a
    /// transport can answer "no credential" distinctly from "not allowed".
    authenticated: bool,
    all: bool,
    allow: Vec<GlobMatcher>,
    deny: Vec<GlobMatcher>,
    /// catalog tokens from the `allow`/`deny` lists, `@` stripped
    allow_cats: Vec<String>,
    deny_cats: Vec<String>,
    allow_pat: Vec<String>,
    deny_pat: Vec<String>,
    scopes: HashMap<String, Vec<String>>,
    rn: Vec<String>,
    /// Filesystem root this principal's processes run under; `None` when the
    /// policy declares none (no directory control). A process's own directory
    /// is `<workdir_root>/<pid>`.
    workdir_root: Option<std::path::PathBuf>,
}

impl Principal {
    /// A principal that passes every check, under the `system` subject.
    ///
    /// Three things reach it, and all of them say so out loud: a disabled ACL
    /// (the explicit opt-out, and the policy a test or a demo runs under), an
    /// `allow = ["*"]` user (an administrator), and the engine's own
    /// operations — the package registrations `Engine` performs while
    /// starting up, which are not requests from anyone.
    pub fn unrestricted() -> Self {
        Self {
            subject: "system".to_string(),
            authenticated: true,
            all: true,
            allow: Vec::new(),
            deny: Vec::new(),
            allow_cats: Vec::new(),
            deny_cats: Vec::new(),
            allow_pat: vec!["*".to_string()],
            deny_pat: Vec::new(),
            scopes: HashMap::new(),
            rn: Vec::new(),
            workdir_root: None,
        }
    }

    /// The anonymous caller under an enabled ACL: no token matched — the
    /// catalogue reads, and nothing else.
    pub fn anonymous() -> Self {
        let allow = compile_patterns(
            &ANONYMOUS_ALLOW
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>(),
            ANONYMOUS_ROLE,
            "allow",
        )
        .expect("the built-in anonymous allow list is valid");
        Self {
            subject: ANONYMOUS_ROLE.to_string(),
            authenticated: false,
            all: false,
            allow,
            deny: Vec::new(),
            allow_cats: Vec::new(),
            deny_cats: Vec::new(),
            allow_pat: ANONYMOUS_ALLOW.iter().map(|a| a.to_string()).collect(),
            deny_pat: Vec::new(),
            scopes: HashMap::new(),
            rn: Vec::new(),
            workdir_root: None,
        }
    }

    /// Compile one user's grants into a principal. Every malformed entry is an
    /// error: a policy that cannot be enforced must be refused when the user
    /// is written, not silently ignored on the way in.
    pub fn from_policy(policy: &UserPolicy) -> Result<Self> {
        let name = policy.name.trim();
        if name.is_empty() {
            return Err(ActError::Config(
                "acl user name cannot be empty".to_string(),
            ));
        }
        // The user name is the subject, and it prefixes the channel key of
        // every subscription the user opens (`{subject}/{client_id}`), so a
        // name carrying the separator could spell another subject's prefix.
        if name.contains('/') {
            return Err(ActError::Config(format!(
                "acl user name '{name}' cannot contain '/'"
            )));
        }
        let all = policy.allow.iter().any(|p| is_all_token(p));
        let (allow, allow_cats) = split_tokens(&policy.allow, name, "allow")?;
        let (deny, deny_cats) = split_tokens(&policy.deny, name, "deny")?;
        for pattern in &policy.patterns {
            Glob::new(pattern.trim())
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    ActError::Config(format!(
                        "acl user '{name}' has an invalid patterns pattern '{pattern}': {err}"
                    ))
                })?;
        }
        for (target, patterns) in &policy.snapshot {
            if target.trim().is_empty() {
                return Err(ActError::Config(format!(
                    "acl user '{name}' has a snapshot rule without a target name"
                )));
            }
            // Validate the patterns now; a bad one must not degrade into
            // "matches nothing" at seal time.
            compile_patterns(patterns, name, "snapshot")?;
        }

        Ok(Self {
            subject: name.to_string(),
            authenticated: true,
            all,
            allow,
            deny,
            allow_cats,
            deny_cats,
            allow_pat: policy.allow.clone(),
            deny_pat: policy.deny.clone(),
            scopes: policy.snapshot.clone(),
            rn: policy.patterns.clone(),
            workdir_root: None,
        })
    }

    /// The principal's user name (its subject).
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Whether this principal is unrestricted (an `allow = ["*"]` user, or a
    /// disabled ACL).
    pub fn is_unrestricted(&self) -> bool {
        self.all
    }

    /// Whether a token resolved to this principal.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    pub fn check(&self, action: &str) -> std::result::Result<(), AclError> {
        // a deny wins — by command pattern or by the action's catalog group
        let catalog = action_catalog(action);
        if self.authenticated
            && (self.deny.iter().any(|glob| glob.is_match(action))
                || self
                    .deny_cats
                    .iter()
                    .any(|cat| cat == "all" || scope_matches(cat, catalog)))
        {
            return Err(AclError::Denied(format!(
                "user '{}' denies action '{action}'",
                self.subject
            )));
        }
        if self.all
            || self.allow.iter().any(|glob| glob.is_match(action))
            || self
                .allow_cats
                .iter()
                .any(|cat| cat == "all" || scope_matches(cat, catalog))
        {
            return Ok(());
        }
        if !self.authenticated {
            // the anonymous caller is held to its catalogue read list above;
            // anything else needs a login, and the transport should say so
            // with "no credential" rather than "not allowed"
            return Err(AclError::Unauthenticated(
                "a session token is required; login first (acl:login)".to_string(),
            ));
        }
        Err(AclError::Denied(format!(
            "action '{action}' is not allowed for user '{}'",
            self.subject
        )))
    }

    pub fn check_scope(&self, target: &str, scope: &str) -> std::result::Result<(), AclError> {
        if !self.authenticated {
            return Err(AclError::Unauthenticated(
                "a session token is required; login first (acl:login)".to_string(),
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

    /// Whether the workflow resource name `rn` is within this principal's
    /// grants (see [`ScopePolicy::allows_rn`]).
    pub fn check_rn(&self, rn: &str) -> std::result::Result<(), AclError> {
        if !self.authenticated {
            return Err(AclError::Unauthenticated(
                "a session token is required; login first (acl:login)".to_string(),
            ));
        }
        if self.scope_policy().allows_rn(rn) {
            return Ok(());
        }
        Err(AclError::Denied(format!(
            "resource '{rn}' is not allowed for user '{}'",
            self.subject
        )))
    }

    /// The scope authority to seal into a process started by this principal.
    pub fn scope_policy(&self) -> ScopePolicy {
        ScopePolicy {
            subject: self.subject.clone(),
            all: self.all,
            scopes: self.scopes.clone(),
            rn: self.rn.clone(),
            // compiled from a caller: its resource patterns decide
            enforce_rn: true,
            workdir_root: self.workdir_root.clone(),
        }
    }

    /// What `acl:whoami` answers: the identity and the patterns in force.
    pub fn to_value(&self) -> JsonValue {
        json!({
            "user": self.subject,
            "subject": self.subject,
            "authenticated": self.authenticated,
            "unrestricted": self.all,
            "allow": self.allow_pat,
            "deny": self.deny_pat,
            "patterns": self.rn,
            "scopes": self.scopes,
            "workdir_root": self.workdir_root.as_ref().map(|dir| dir.display().to_string()),
        })
    }
}

/// The engine's access control, as the engine uses it: authenticate a request,
/// log in and out, and manage the users behind it.
///
/// The implementation owns the users and the live sessions — their storage,
/// their passwords and their expiry. `acts-acl`'s `UserAcl` is the shipped
/// one, keeping its rows in the engine's store; [`AnonymousAcl`] is what an
/// engine without one runs, and [`DisabledAcl`] is the opt-out.
#[async_trait::async_trait]
pub trait AccessControl: Send + Sync + fmt::Debug {
    /// Bind to the engine's store and load the users and live sessions, and
    /// bootstrap whatever an empty registry needs. Runs once at engine start,
    /// before any transport serves.
    async fn load(&self, store: Arc<Store>) -> Result<()>;

    /// Whether enforcement is on at all. A disabled ACL passes every caller
    /// through as [`Principal::unrestricted`].
    fn enabled(&self) -> bool;

    /// Resolve a request's token into a principal. An absent, unknown or
    /// expired token resolves to the anonymous catalogue-only principal.
    fn authenticate(&self, token: Option<&str>) -> std::result::Result<Principal, AclError>;

    /// The principal a request without a valid token resolves to.
    fn anonymous(&self) -> Principal {
        self.authenticate(None)
            .unwrap_or_else(|_| Principal::anonymous())
    }

    /// `acl:login`: verify `user`/`password` and mint a token pair.
    async fn login(&self, user: &str, password: &str)
    -> std::result::Result<LoginTokens, AclError>;

    /// `acl:refresh`: rotate the session a refresh token belongs to.
    async fn refresh(&self, refresh_token: &str) -> std::result::Result<LoginTokens, AclError>;

    /// `acl:logout`: drop the session `token` belongs to.
    async fn logout(&self, token: &str) -> Result<bool>;

    /// `acl:setuser`: create or update one user.
    async fn set_user(&self, spec: &UserSpec) -> Result<()>;

    /// `acl:deluser`: delete one user.
    async fn del_user(&self, name: &str) -> Result<()>;

    /// `acl:users`: the user names, sorted.
    async fn user_names(&self) -> Vec<String>;

    /// `acl:getuser`: one user's policy, without the password hashes.
    async fn get_user(&self, name: &str) -> Option<JsonValue>;
}

/// The access control of an engine without an implementation installed: no
/// users, no sessions, no login — every caller is the anonymous
/// catalogue-only principal. It is what a bare `Engine::builder().start()`
/// runs, so an embedder that never configures users cannot be reached.
#[derive(Debug, Default)]
pub struct AnonymousAcl;

#[async_trait::async_trait]
impl AccessControl for AnonymousAcl {
    async fn load(&self, _store: Arc<Store>) -> Result<()> {
        Ok(())
    }

    fn enabled(&self) -> bool {
        true
    }

    fn authenticate(&self, _token: Option<&str>) -> std::result::Result<Principal, AclError> {
        Ok(Principal::anonymous())
    }

    async fn login(
        &self,
        _user: &str,
        _password: &str,
    ) -> std::result::Result<LoginTokens, AclError> {
        Err(AclError::Denied(
            "no user registry is installed in this engine".to_string(),
        ))
    }

    async fn refresh(&self, _refresh_token: &str) -> std::result::Result<LoginTokens, AclError> {
        Err(AclError::Denied(
            "no user registry is installed in this engine".to_string(),
        ))
    }

    async fn logout(&self, _token: &str) -> Result<bool> {
        Ok(false)
    }

    async fn set_user(&self, _spec: &UserSpec) -> Result<()> {
        Err(ActError::Config(
            "no user registry is installed in this engine".to_string(),
        ))
    }

    async fn del_user(&self, _name: &str) -> Result<()> {
        Err(ActError::Config(
            "no user registry is installed in this engine".to_string(),
        ))
    }

    async fn user_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn get_user(&self, _name: &str) -> Option<JsonValue> {
        None
    }
}

/// An explicitly disabled ACL — no enforcement, every operation passes with
/// [`Principal::unrestricted`]. Only reachable through
/// [`crate::EngineBuilder::disable_acl`], spelled out at the call site.
#[derive(Debug, Default)]
pub struct DisabledAcl;

#[async_trait::async_trait]
impl AccessControl for DisabledAcl {
    async fn load(&self, _store: Arc<Store>) -> Result<()> {
        Ok(())
    }

    fn enabled(&self) -> bool {
        false
    }

    fn authenticate(&self, _token: Option<&str>) -> std::result::Result<Principal, AclError> {
        Ok(Principal::unrestricted())
    }

    async fn login(
        &self,
        _user: &str,
        _password: &str,
    ) -> std::result::Result<LoginTokens, AclError> {
        Err(AclError::Denied(
            "the acl is disabled: requests need no login".to_string(),
        ))
    }

    async fn refresh(&self, _refresh_token: &str) -> std::result::Result<LoginTokens, AclError> {
        Err(AclError::Denied(
            "the acl is disabled: requests need no login".to_string(),
        ))
    }

    async fn logout(&self, _token: &str) -> Result<bool> {
        Ok(false)
    }

    async fn set_user(&self, _spec: &UserSpec) -> Result<()> {
        Err(ActError::Config(
            "the acl is disabled: there is no user registry".to_string(),
        ))
    }

    async fn del_user(&self, _name: &str) -> Result<()> {
        Err(ActError::Config(
            "the acl is disabled: there is no user registry".to_string(),
        ))
    }

    async fn user_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn get_user(&self, _name: &str) -> Option<JsonValue> {
        None
    }
}

fn compile_patterns(patterns: &[String], user: &str, field: &str) -> Result<Vec<GlobMatcher>> {
    patterns
        .iter()
        .map(|pattern| {
            Glob::new(pattern)
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    ActError::Config(format!(
                        "acl user '{user}' has an invalid {field} pattern '{pattern}': {err}"
                    ))
                })
        })
        .collect()
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

    fn policy(name: &str) -> UserPolicy {
        UserPolicy {
            name: name.to_string(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn catalog_groups_classify_actions() {
        assert_eq!(action_catalog("model:ls"), "read");
        assert_eq!(action_catalog("msg:sub"), "read");
        assert_eq!(action_catalog("acl:getuser"), "read");
        assert_eq!(action_catalog("model:deploy"), "deploy");
        assert_eq!(action_catalog("pack:publish"), "deploy");
        assert_eq!(action_catalog("proc:start"), "execute");
        assert_eq!(action_catalog("proc:start_from_model"), "execute");
        assert_eq!(action_catalog("act:complete"), "execute");
        assert_eq!(action_catalog("msg:ack"), "execute");
        assert_eq!(action_catalog("model:rm"), "write");
        assert_eq!(action_catalog("snap:upsert"), "write");
        assert_eq!(action_catalog("acl:setuser"), "write");
        assert_eq!(action_catalog("acl:login"), "write");
        assert_eq!(action_catalog("msg:unsub"), "write");
        // an unknown action is never a read
        assert_eq!(action_catalog("future:thing"), "write");
    }

    #[test]
    fn a_policy_compiles_its_grants() {
        let principal = Principal::from_policy(&UserPolicy {
            name: "operator".to_string(),
            enabled: true,
            allow: vec![
                "@read".to_string(),
                "@execute".to_string(),
                "pack:publish".to_string(),
            ],
            deny: vec!["@write".to_string(), "msg:ack".to_string()],
            patterns: vec!["orders:*".to_string()],
            snapshot: HashMap::from([("secrets".to_string(), vec!["$subject".to_string()])]),
        })
        .unwrap();

        assert!(!principal.is_unrestricted());
        // @read covers the reads
        principal.check("model:ls").unwrap();
        principal.check("task:get").unwrap();
        principal.check("acl:whoami").unwrap();
        // @execute covers starting and driving runs...
        principal.check("proc:start").unwrap();
        principal.check("act:complete").unwrap();
        // ...and an explicit deny wins over the group grant that covers it
        assert!(principal.check("msg:ack").is_err());
        // a command pattern names one action of a group not granted
        principal.check("pack:publish").unwrap();
        assert!(principal.check("model:deploy").is_err());
        // @write denies every removal and feed, named or not
        assert!(principal.check("model:rm").is_err());
        assert!(principal.check("snap:upsert").is_err());
        assert!(principal.check("acl:setuser").is_err());
        // resources
        principal.check_rn("orders:eu").unwrap();
        assert!(principal.check_rn("billing:eu").is_err());
        assert!(
            principal.check_rn("").is_err(),
            "a model that claims no resource is outside every pattern"
        );
        // snapshot scopes
        let policy = principal.scope_policy();
        assert!(policy.allows("secrets", "operator"));
        assert!(!policy.allows("secrets", "someone-else"));
    }

    #[test]
    fn an_all_token_is_unrestricted() {
        let principal = Principal::from_policy(&UserPolicy {
            allow: vec!["@all".to_string()],
            ..policy("root")
        })
        .unwrap();
        assert!(principal.is_unrestricted());
        principal.check("model:rm").unwrap();
        principal.check("acl:setuser").unwrap();
        principal.check_rn("").unwrap();

        // `*` and `@*` are the same grant
        for token in ["*", "@*"] {
            let principal = Principal::from_policy(&UserPolicy {
                allow: vec![token.to_string()],
                ..policy("root")
            })
            .unwrap();
            assert!(principal.is_unrestricted(), "{token}");
        }

        // ...and @all in deny refuses everything
        let principal = Principal::from_policy(&UserPolicy {
            allow: vec!["*".to_string()],
            deny: vec!["@all".to_string()],
            ..policy("nobody")
        })
        .unwrap();
        assert!(principal.check("model:ls").is_err());
    }

    #[test]
    fn invalid_names_patterns_and_catalog_tokens_are_refused() {
        // a name carrying the separator could spell another subject's prefix
        assert!(
            Principal::from_policy(&UserPolicy {
                name: "a/b".to_string(),
                enabled: true,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            Principal::from_policy(&UserPolicy {
                name: String::new(),
                ..Default::default()
            })
            .is_err()
        );
        // a malformed pattern would silently match nothing
        assert!(
            Principal::from_policy(&UserPolicy {
                allow: vec!["[invalid".to_string()],
                ..policy("x")
            })
            .is_err()
        );
        assert!(
            Principal::from_policy(&UserPolicy {
                patterns: vec!["[invalid".to_string()],
                ..policy("x")
            })
            .is_err()
        );
        // a catalog token that names no group grants nothing: refuse the typo
        let err = Principal::from_policy(&UserPolicy {
            allow: vec!["@reed".to_string()],
            ..policy("x")
        })
        .unwrap_err();
        assert!(err.to_string().contains("names no catalog group"), "{err}");
        // an empty snapshot target names nothing either
        assert!(
            Principal::from_policy(&UserPolicy {
                snapshot: HashMap::from([(String::new(), vec!["$subject".to_string()])]),
                ..policy("x")
            })
            .is_err()
        );
    }

    #[test]
    fn the_anonymous_principal_reads_the_catalogue_only() {
        let principal = Principal::anonymous();
        assert!(!principal.is_authenticated());
        principal.check("model:ls").unwrap();
        principal.check("pack:get").unwrap();
        assert!(principal.check("model:deploy").is_err());
        assert!(principal.check("proc:start").is_err());
        assert!(principal.check("acl:setuser").is_err());
        assert!(principal.check_rn("orders:eu").is_err());
        // every refusal beyond the catalogue is "log in", not "not allowed"
        assert!(matches!(
            principal.check("model:deploy"),
            Err(AclError::Unauthenticated(_))
        ));
    }

    #[test]
    fn scope_policies_deny_by_default() {
        let policy = ScopePolicy::default();
        assert!(!policy.allows("secrets", "alice"));
        // ...but an internal policy decides no resources: nobody named them
        assert!(policy.allows_rn("orders:eu"));
        assert!(policy.allows_rn(""));

        let policy = ScopePolicy::unrestricted();
        assert!(policy.allows("secrets", "alice"));
        assert!(policy.allows_rn(""));

        // a caller's policy decides resources, and refuses what no pattern
        // covers (including a model that claims none)
        let policy = Principal::from_policy(&UserPolicy {
            name: "op".to_string(),
            enabled: true,
            patterns: vec!["orders:*".to_string()],
            ..Default::default()
        })
        .unwrap()
        .scope_policy();
        assert!(policy.allows_rn("orders:eu"));
        assert!(!policy.allows_rn(""));
        assert!(!policy.allows_rn("billing:eu"));
    }

    #[test]
    fn an_anonymous_acl_answers_no_login_and_no_users() {
        let acl = AnonymousAcl;
        assert!(acl.enabled());
        assert!(
            !acl.authenticate(Some("anything"))
                .unwrap()
                .is_authenticated()
        );
        assert!(!acl.anonymous().is_authenticated());
    }

    #[test]
    fn a_disabled_acl_passes_everything() {
        let acl = DisabledAcl;
        assert!(!acl.enabled());
        let principal = acl.authenticate(None).unwrap();
        assert!(principal.is_unrestricted());
        principal.check("model:rm").unwrap();
    }
}
