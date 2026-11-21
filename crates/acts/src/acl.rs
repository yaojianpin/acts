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
//!   token (or with an unknown or expired one) is refused — the
//!   [`ANONYMOUS_ROLE`] principal holds no grant; logging in is the way in.
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
use std::collections::{BTreeMap, HashMap};
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
/// `acl:logout_user` — an administrator revokes **every** live session of one
/// user (`{user}`): the user's next request is refused, while its passwords
/// and grants stay untouched, so it can log in again. Needs an ordinary write
/// grant (it is user administration, like `acl:setuser`).
pub const ACTION_LOGOUT_USER: &str = "acl:logout_user";
/// `acl:disable_user` — an administrator turns login off for one user
/// (`{user}`): its live sessions are revoked and `acl:login` refuses it until
/// it is enabled again. The builtin `admin` cannot be disabled.
pub const ACTION_DISABLE_USER: &str = "acl:disable_user";
/// `acl:enable_user` — an administrator turns login back on for one user
/// (`{user}`).
pub const ACTION_ENABLE_USER: &str = "acl:enable_user";
/// Whether `action` carries its own credential in its payload, so a request
/// whose bearer no longer resolves must still reach it: `acl:login` (user +
/// password), `acl:refresh` (the refresh token) and `acl:logout` (the session
/// token). Refusing these at the transport would make the very request that
/// repairs a dead session impossible; for every other action the bearer is
/// the credential, and an invalid one is refused as such.
pub fn carries_own_credential(action: &str) -> bool {
    matches!(action, ACTION_LOGIN | ACTION_REFRESH | ACTION_LOGOUT)
}
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

/// Every catalog group a user's `allow`/`deny` list may name, plus the
/// catch-all. A token that matches none of them is a config error rather
/// than a grant (or a denial) that silently does nothing.
pub const CATALOG_GROUPS: &[&str] = &["read", "write", "deploy", "execute", "assign", "all"];

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
    /// Organization-dimension vocabulary (`tenant`, `unit`, `department`, …)
    /// the user's patterns may substitute as `$ctx.<key>`. Keys are
    /// `[a-z][a-z0-9_]{0,31}`, values non-empty and at most 128 bytes, at
    /// most 16 entries (see [`validate_context`]).
    #[serde(default)]
    pub context: BTreeMap<String, String>,
}

/// Validate one user's context vocabulary. A bad entry is a config error:
/// a pattern that substitutes a value the policy does not carry must be
/// refused when the user is written, not silently fail closed at seal time.
pub fn validate_context(context: &BTreeMap<String, String>) -> Result<()> {
    const MAX_ENTRIES: usize = 16;
    const MAX_VALUE_BYTES: usize = 128;
    if context.len() > MAX_ENTRIES {
        return Err(ActError::Config(format!(
            "acl user context carries {} entries; at most {MAX_ENTRIES} are allowed",
            context.len()
        )));
    }
    for (key, value) in context {
        let valid_key = {
            let mut chars = key.chars();
            match chars.next() {
                Some(first) if first.is_ascii_lowercase() => {
                    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                        && key.len() <= 32
                }
                _ => false,
            }
        };
        if !valid_key {
            return Err(ActError::Config(format!(
                "acl user context key '{key}' is invalid: it must match [a-z][a-z0-9_]{{0,31}}"
            )));
        }
        if value.is_empty() || value.len() > MAX_VALUE_BYTES {
            return Err(ActError::Config(format!(
                "acl user context key '{key}' has an invalid value: it must be 1..={MAX_VALUE_BYTES} bytes"
            )));
        }
    }
    Ok(())
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
    /// Force every live session of the user out by bumping its security
    /// version — the O(1) "log this user out everywhere" switch.
    pub force_logout: Option<bool>,
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
    /// Organization-dimension vocabulary (`$ctx.<key>` substitution source);
    /// replaces the current map. `None` leaves it unchanged.
    #[serde(default)]
    pub context: Option<BTreeMap<String, String>>,
}

/// What `acl:login` and `acl:refresh` answer.
#[derive(Clone, Serialize, Deserialize)]
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

/// A token is a secret: `Debug` shows its shape (the `at_`/`rt_` prefix and
/// its length) so log lines stay useful without carrying a usable credential.
impl std::fmt::Debug for LoginTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginTokens")
            .field("token", &mask_token(&self.token))
            .field("refresh_token", &mask_token(&self.refresh_token))
            .field("expires_in", &self.expires_in)
            .field("refresh_expires_in", &self.refresh_expires_in)
            .finish()
    }
}

fn mask_token(token: &str) -> String {
    let head: String = token.chars().take(6).collect();
    format!("{head}…(len={})", token.chars().count())
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
    /// root — is what `Scope::workdir` and `Context::workdir` answer, and
    /// what `$env.WORK_DIR` names). Compiled from the engine config (see
    /// [`crate::Config::workdir`]); `None` means no directory control.
    #[serde(default)]
    pub workdir_root: Option<std::path::PathBuf>,
    /// The organization vocabulary sealed with the run: the substitution
    /// source for `$ctx.<key>` in the scope patterns below. It travels with
    /// the authority (`#owner`), so a run reads patterns against the context
    /// its starter carried — later policy edits never widen a live run.
    #[serde(default)]
    pub context: BTreeMap<String, String>,
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
            context: BTreeMap::new(),
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
            context: BTreeMap::new(),
        }
    }

    /// Whether `scope` of `target` is within this policy.
    pub fn allows(&self, target: &str, scope: &str) -> bool {
        if self.all {
            return true;
        }
        match self.scopes.get(target) {
            Some(patterns) => patterns.iter().any(|pattern| {
                substitute(pattern, &self.subject, &self.context)
                    .is_some_and(|pattern| scope_matches(&pattern, scope))
            }),
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
        self.rn.iter().any(|pattern| {
            substitute(pattern, &self.subject, &self.context)
                .is_some_and(|pattern| scope_matches(&pattern, rn))
        })
    }
}

/// The catalog (command group) an action belongs to — the `@category` of a
/// user's `allow`/`deny` list. Five groups cover the action set, plus `all`:
///
/// - `read` — the catalogue and row reads, `acl:whoami`/`acl:getuser`/
///   `acl:users`, and `msg:sub` (a subscription is a read of the stream)
/// - `deploy` — putting work into the catalogues: `model:deploy`,
///   `pack:publish`
/// - `execute` — running and driving that work: `proc:start`,
///   `proc:start_from_model`, `evt:start`, the decision `act:*` actions, and
///   `msg:ack`
/// - `assign` — managing who decides a human act: `act:transfer`,
///   `act:delegate`, `act:undelegate`. It is deliberately separate from
///   `@execute`: running a flow must not imply moving someone else's approval,
///   and a task's owner may manage its assignment without either grant.
/// - `write` — changing or destroying stored state: every `*:rm`,
///   `msg:redo`/`msg:redo_all`/`msg:clear`, the snapshot feeds, and the user management —
///   plus `sys:status`, a read that is kept here on purpose (it echoes the
///   server's own configuration, which may carry credentials; see below)
/// - `all` — every action (`@all` in a list is the unrestricted grant, like
///   `*`)
///
/// An action not named here is `write`: an unknown operation is never a read.
/// `sys:status` is deliberate: it answers nothing of anyone's work, but its
/// config section echoes the server's own configuration — a `[db]`
/// `database_url` may carry credentials — so it stays out of `@read` and an
/// operator grants it (`--allow sys:status`, or `@all`) to whom may see it.
pub fn action_catalog(action: &str) -> &'static str {
    match action {
        "sys:status" => "write",
        "model:ls" | "model:get" | "pack:ls" | "pack:get" | "proc:ls" | "proc:get"
        | "proc:graph" | "task:ls" | "task:get" | "msg:ls" | "msg:get" | "evt:ls" | "evt:get"
        | "snap:get" | "snap:ls" | ACTION_WHOAMI | ACTION_GETUSER | ACTION_USERS
        | ACTION_SUBSCRIBE => "read",
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
        | "act:remind"
        | "msg:ack" => "execute",
        // Reassigning an act is not running it: the capability is separate
        // from `@execute` so a user who may drive a flow cannot move someone
        // else's approval to themselves. Delegating and revoking a delegation
        // are the same management surface.
        "act:transfer" | "act:delegate" | "act:undelegate" => "assign",
        _ => "write",
    }
}

/// The request gate for one action.
///
/// Every action is decided by [`Principal::check`], except a per-task action
/// (`act:transfer`, the `assign` group): its authority is the task's owner or
/// the caller's `assign` capability, so an authenticated caller passes this
/// global gate and the kernel decides on the task state. An unauthenticated
/// caller has no identity the kernel could authorize and is refused here.
pub fn check_action(principal: &Principal, action: &str) -> std::result::Result<(), AclError> {
    if action_catalog(action) != "assign" || !principal.is_authenticated() {
        principal.check(action)?;
    }
    Ok(())
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
                     catalog group (read, write, deploy, execute, assign, all)"
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
    /// The organization vocabulary the principal's patterns substitute
    /// (`$ctx.<key>`); travels into every [`Self::scope_policy`] it seals.
    context: BTreeMap<String, String>,
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
            context: BTreeMap::new(),
        }
    }

    /// The anonymous caller under an enabled ACL: no token matched. It holds
    /// **no** grant — not even the catalogue reads — so every action but the
    /// three that carry their own credential (`acl:login`, `acl:refresh`,
    /// `acl:logout`, and the identity probe `acl:whoami`) refuses it; logging
    /// in is the way in.
    pub fn anonymous() -> Self {
        Self {
            subject: ANONYMOUS_ROLE.to_string(),
            authenticated: false,
            all: false,
            allow: Vec::new(),
            deny: Vec::new(),
            allow_cats: Vec::new(),
            deny_cats: Vec::new(),
            allow_pat: Vec::new(),
            deny_pat: Vec::new(),
            scopes: HashMap::new(),
            rn: Vec::new(),
            workdir_root: None,
            context: BTreeMap::new(),
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
        validate_context(&policy.context)?;

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
            context: policy.context.clone(),
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
            // the anonymous principal holds no grant at all; every refusal is
            // "log in", never "not allowed"
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
            context: self.context.clone(),
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
            "context": self.context,
        })
    }
}

/// What [`AccessControl::resolve`] decided about a request's credential.
///
/// Three states, kept apart on purpose: "no credential" is routed by route
/// policy, "a credential that checks out" proceeds, and "a credential that
/// does not" is refused with 401/UNAUTHENTICATED — never folded into the
/// anonymous caller, which is what turned a forged token into a 403 before
/// (audit ACL-AUTH-01).
//
// The authenticated variant carries the whole compiled principal (≈280 bytes)
// and is passed by value exactly once per request, never stored; boxing it
// would turn every authenticated request into one heap allocation purely to
// shrink a stack temporary that dies at the end of the match.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AuthenticationResult {
    /// The request carried no token: the grant-free anonymous principal, so
    /// only the actions that carry their own credential and `acl:whoami`
    /// answer it.
    MissingCredentials,
    /// The token resolved to a live session.
    Authenticated(Principal),
    /// The request carried a token that resolves to nothing: unknown,
    /// expired, consumed, revoked, or its subject no longer matches. The
    /// transports refuse it with 401/UNAUTHENTICATED, except for the actions
    /// that carry their own credential in the payload
    /// ([`carries_own_credential`]) — those stay reachable, or a caller could
    /// never log in or rotate a session again.
    InvalidCredentials,
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

    /// Whether `subject` must change its password before anything else: the
    /// engine then refuses every action except the password change itself
    /// (`acl:setuser` naming the subject with a new password); the change
    /// bumps the security version, so the next request must log in again.
    /// `false` — the default — means no initial-state account exists.
    fn password_change_required(&self, subject: &str) -> bool {
        let _ = subject;
        false
    }

    /// Whether the implementation keeps a user registry an authorization
    /// question can be asked of. `false` — the anonymous registry knows no
    /// users — keeps the fetch-request gate open: channel filters are then
    /// all that decides who sees a request (an engine without users has no
    /// subject scopes to leak).
    fn manages_users(&self) -> bool {
        false
    }

    /// The compiled principal of a stored user — **not** of a session: what
    /// a subscription under `subject` may read. `None` — unknown or disabled
    /// user. Answered only by the fetch-request gate (`Emitter::emit_req`
    /// withholds a request from channels whose subject may not read the
    /// scope it names); a request carries no data, but its scope name need
    /// not reach subjects that cannot read it.
    async fn principal_of(&self, subject: &str) -> Option<Principal> {
        let _ = subject;
        None
    }

    /// Resolve a request's token into a principal. An absent, unknown or
    /// expired token resolves to the grant-free anonymous principal.
    fn authenticate(&self, token: Option<&str>) -> std::result::Result<Principal, AclError>;

    /// Resolve a request's token into one of three states (see
    /// [`AuthenticationResult`]). The default maps a legacy
    /// [`AccessControl::authenticate`] implementation onto the states; an
    /// implementation that tracks session state precisely overrides it.
    async fn resolve(&self, token: Option<&str>) -> AuthenticationResult {
        match token.map(str::trim).filter(|token| !token.is_empty()) {
            None => AuthenticationResult::MissingCredentials,
            Some(token) => match self.authenticate(Some(token)) {
                Ok(principal) if principal.is_authenticated() => {
                    AuthenticationResult::Authenticated(principal)
                }
                _ => AuthenticationResult::InvalidCredentials,
            },
        }
    }

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

    /// `acl:logout_user`: revoke every live session of `name` — the
    /// administrator's "log this user out everywhere" — without touching its
    /// passwords or grants, so it can log in again. Answers whether the user
    /// existed. The default refuses: a registry that cannot revoke by user
    /// must not report a logout that did not happen.
    async fn logout_user(&self, name: &str) -> Result<bool> {
        Err(ActError::Action(format!(
            "this access control cannot log out user '{name}'"
        )))
    }

    /// `acl:disable_user` / `acl:enable_user`: decide whether `name` may log
    /// in. Disabling revokes the sessions the user holds (the security
    /// version moves); enabling only reopens the login. Answers whether the
    /// user existed; an implementation that cannot change the state refuses.
    async fn set_user_enabled(&self, name: &str, enabled: bool) -> Result<bool> {
        let _ = enabled;
        Err(ActError::Action(format!(
            "this access control cannot enable or disable user '{name}'"
        )))
    }

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
/// users, no sessions, no login — every caller is the grant-free anonymous
/// principal, so nothing but `acl:whoami` answers (and `acl:login` refuses
/// because there is no registry). It is what a bare
/// `Engine::builder().start()` runs, so an embedder that never configures
/// users cannot be reached.
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

    async fn logout_user(&self, _name: &str) -> Result<bool> {
        Err(ActError::Config(
            "no user registry is installed in this engine".to_string(),
        ))
    }

    async fn set_user_enabled(&self, _name: &str, _enabled: bool) -> Result<bool> {
        Err(ActError::Config(
            "no user registry is installed in this engine".to_string(),
        ))
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

    async fn logout_user(&self, _name: &str) -> Result<bool> {
        Err(ActError::Config(
            "the acl is disabled: there is no user registry".to_string(),
        ))
    }

    async fn set_user_enabled(&self, _name: &str, _enabled: bool) -> Result<bool> {
        Err(ActError::Config(
            "the acl is disabled: there is no user registry".to_string(),
        ))
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

/// Replace every `$subject` and `$ctx.<key>` in a pattern.
///
/// `None` means the pattern names a context key the policy does not carry:
/// the pattern must then match **nothing** (fail-closed). Substituting an
/// empty string instead could let `""` combine with the rest of the pattern
/// into a scope it was never meant to name. `$subject` is always present.
fn substitute(pattern: &str, subject: &str, context: &BTreeMap<String, String>) -> Option<String> {
    if !pattern.contains('$') {
        return Some(pattern.to_string());
    }
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(idx) = rest.find('$') {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx + 1..];
        if let Some(after) = tail.strip_prefix("subject") {
            out.push_str(subject);
            rest = after;
        } else if let Some(after) = tail.strip_prefix("ctx.") {
            let key = after
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or("");
            if key.is_empty() {
                // a malformed `$ctx.` token names no key: it must not
                // silently degrade into a literal or an empty substitution
                return None;
            }
            let value = context.get(key)?;
            out.push_str(value);
            rest = &after[key.len()..];
        } else {
            // not our token: keep the `$` verbatim
            out.push('$');
            rest = tail;
        }
    }
    out.push_str(rest);
    Some(out)
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
        assert_eq!(action_catalog("proc:graph"), "read");
        assert_eq!(action_catalog("model:deploy"), "deploy");
        assert_eq!(action_catalog("pack:publish"), "deploy");
        assert_eq!(action_catalog("proc:start"), "execute");
        assert_eq!(action_catalog("proc:start_from_model"), "execute");
        assert_eq!(action_catalog("act:complete"), "execute");
        assert_eq!(action_catalog("act:transfer"), "assign");
        assert_eq!(action_catalog("act:delegate"), "assign");
        assert_eq!(action_catalog("act:undelegate"), "assign");
        assert_eq!(action_catalog("msg:ack"), "execute");
        assert_eq!(action_catalog("msg:redo"), "write");
        assert_eq!(action_catalog("msg:redo_all"), "write");
        assert_eq!(action_catalog("model:rm"), "write");
        assert_eq!(action_catalog("snap:upsert"), "write");
        assert_eq!(action_catalog("snap:reply"), "write");
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
            ..Default::default()
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
        // `@assign` is independent of `@execute`: this policy holds neither,
        // and a transfer is out of reach.
        assert!(principal.check("act:transfer").is_err());
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
    fn assign_is_its_own_capability() {
        let assigner = Principal::from_policy(&UserPolicy {
            allow: vec!["@assign".to_string()],
            ..policy("assigner")
        })
        .unwrap();
        assigner.check("act:transfer").unwrap();
        assert!(
            assigner.check("act:complete").is_err(),
            "@assign must not imply running acts"
        );

        let operator = Principal::from_policy(&UserPolicy {
            allow: vec!["@execute".to_string()],
            ..policy("operator")
        })
        .unwrap();
        operator.check("act:complete").unwrap();
        assert!(
            operator.check("act:transfer").is_err(),
            "@execute must not imply reassigning acts"
        );
    }

    #[test]
    fn per_task_actions_pass_the_global_gate_only_when_authenticated() {
        let owner = Principal::from_policy(&policy("alice")).unwrap();
        // An authenticated caller reaches the kernel's task check even with
        // no command grant; an unauthenticated one has no identity to check.
        check_action(&owner, "act:transfer").unwrap();
        assert!(check_action(&owner, "act:complete").is_err());
        assert!(check_action(&Principal::anonymous(), "act:transfer").is_err());

        // A global grant still passes the gate; the kernel decides ownership.
        let operator = Principal::from_policy(&UserPolicy {
            allow: vec!["@execute".to_string()],
            ..policy("operator")
        })
        .unwrap();
        check_action(&operator, "act:transfer").unwrap();
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
    fn the_anonymous_principal_holds_no_grant() {
        let principal = Principal::anonymous();
        assert!(!principal.is_authenticated());
        // not even the catalogue reads: everything is "log in", never a grant
        for action in ["model:ls", "model:get", "pack:get", "pack:ls"] {
            assert!(
                matches!(principal.check(action), Err(AclError::Unauthenticated(_))),
                "'{action}' must need a credential"
            );
        }
        assert!(matches!(
            principal.check("model:deploy"),
            Err(AclError::Unauthenticated(_))
        ));
        assert!(matches!(
            principal.check("acl:setuser"),
            Err(AclError::Unauthenticated(_))
        ));
        assert!(principal.check_rn("orders:eu").is_err());
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

    #[tokio::test]
    async fn an_anonymous_acl_answers_no_login_and_no_users() {
        let acl = AnonymousAcl;
        assert!(acl.enabled());
        // no user registry: the fetch-request gate stays open, channel
        // filters are all that decides who sees a request
        assert!(!acl.manages_users());
        assert!(acl.principal_of("op").await.is_none());
        assert!(
            !acl.authenticate(Some("anything"))
                .unwrap()
                .is_authenticated()
        );
        assert!(!acl.anonymous().is_authenticated());
    }

    #[tokio::test]
    async fn a_disabled_acl_passes_everything() {
        let acl = DisabledAcl;
        assert!(!acl.enabled());
        assert!(!acl.manages_users());
        let principal = acl.authenticate(None).unwrap();
        assert!(principal.is_unrestricted());
        principal.check("model:rm").unwrap();
    }

    #[test]
    fn acl_error_display_and_act_error_conversions() {
        let unauth = AclError::Unauthenticated("no token".to_string());
        assert_eq!(unauth.to_string(), "unauthenticated: no token");
        let denied = AclError::Denied("nope".to_string());
        assert_eq!(denied.to_string(), "denied: nope");

        let err: ActError = AclError::Unauthenticated("u".to_string()).into();
        assert!(matches!(err, ActError::Unauthenticated(_)));
        let err: ActError = AclError::Denied("d".to_string()).into();
        assert!(matches!(err, ActError::Denied(_)));

        let err: AclError = ActError::Unauthenticated("u".to_string()).into();
        assert!(matches!(err, AclError::Unauthenticated(_)));
        let err: AclError = ActError::Denied("d".to_string()).into();
        assert!(matches!(err, AclError::Denied(_)));
        let err: AclError = ActError::Config("bad".to_string()).into();
        assert!(matches!(err, AclError::Denied(_)));
        assert!(err.to_string().contains("bad"));
    }

    #[test]
    fn an_empty_catalog_token_is_refused() {
        let err = Principal::from_policy(&UserPolicy {
            allow: vec!["@".to_string()],
            ..policy("x")
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("empty allow catalog token"),
            "{err}"
        );
    }

    #[test]
    fn an_invalid_snapshot_pattern_is_refused() {
        let err = Principal::from_policy(&UserPolicy {
            snapshot: HashMap::from([("secrets".to_string(), vec!["[invalid".to_string()])]),
            ..policy("x")
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid snapshot pattern"),
            "{err}"
        );
    }

    #[test]
    fn an_unauthenticated_scope_and_rn_check_asks_for_login() {
        let principal = Principal::anonymous();
        assert!(matches!(
            principal.check_scope("secrets", "anonymous"),
            Err(AclError::Unauthenticated(_))
        ));
        assert!(matches!(
            principal.check_rn("orders:eu"),
            Err(AclError::Unauthenticated(_))
        ));
    }

    #[tokio::test]
    async fn anonymous_acl_async_methods_refuse_everything() {
        let acl = AnonymousAcl;
        assert!(matches!(
            acl.login("u", "p").await,
            Err(AclError::Denied(_))
        ));
        assert!(matches!(acl.refresh("r").await, Err(AclError::Denied(_))));
        assert!(!acl.logout("t").await.unwrap());
        assert!(acl.set_user(&UserSpec::default()).await.is_err());
        assert!(acl.del_user("u").await.is_err());
        assert!(acl.user_names().await.is_empty());
        assert!(acl.get_user("u").await.is_none());
    }

    #[tokio::test]
    async fn disabled_acl_async_methods_refuse_registry_work() {
        let acl = DisabledAcl;
        assert!(matches!(
            acl.login("u", "p").await,
            Err(AclError::Denied(_))
        ));
        assert!(matches!(acl.refresh("r").await, Err(AclError::Denied(_))));
        assert!(!acl.logout("t").await.unwrap());
        assert!(acl.set_user(&UserSpec::default()).await.is_err());
        assert!(acl.del_user("u").await.is_err());
        assert!(acl.user_names().await.is_empty());
        assert!(acl.get_user("u").await.is_none());
    }

    #[test]
    fn substitute_replaces_subject_and_context_keys() {
        let ctx = BTreeMap::from([
            ("tenant".to_string(), "t1".to_string()),
            ("unit".to_string(), "u9".to_string()),
        ]);
        // fast path: no `$` at all
        assert_eq!(substitute("plain", "u", &ctx).as_deref(), Some("plain"));
        // `$subject`
        assert_eq!(
            substitute("$subject/own", "alice", &ctx).as_deref(),
            Some("alice/own")
        );
        // `$ctx.<key>`
        assert_eq!(
            substitute("$ctx.tenant/$ctx.unit/*", "alice", &ctx).as_deref(),
            Some("t1/u9/*")
        );
        // mixed with `$subject`
        assert_eq!(
            substitute("$subject@$ctx.tenant", "bob", &ctx).as_deref(),
            Some("bob@t1")
        );
        // a `$` that names neither token stays verbatim
        assert_eq!(substitute("a$b", "u", &ctx).as_deref(), Some("a$b"));
        assert_eq!(substitute("$$", "u", &ctx).as_deref(), Some("$$"));
    }

    #[test]
    fn substitute_fails_closed_on_absent_or_malformed_context_keys() {
        let ctx = BTreeMap::from([("tenant".to_string(), "t1".to_string())]);
        // a key the policy does not carry: the pattern matches nothing
        assert_eq!(substitute("$ctx.unit/*", "u", &ctx), None);
        // a malformed `$ctx.` token names no key: fail closed, never literal
        assert_eq!(substitute("$ctx./x", "u", &ctx), None);
        assert_eq!(substitute("$ctx.", "u", &ctx), None);
        // an empty context denies every `$ctx.` pattern
        assert_eq!(substitute("$ctx.tenant", "u", &BTreeMap::new()), None);
    }

    #[test]
    fn context_patterns_confine_scopes_per_tenant() {
        let owner = |tenant: &str| {
            Principal::from_policy(&UserPolicy {
                name: format!("op-{tenant}"),
                enabled: true,
                snapshot: HashMap::from([(
                    "profile".to_string(),
                    vec!["$ctx.tenant/$ctx.unit/*".to_string()],
                )]),
                patterns: vec!["orders:$ctx.tenant:*".to_string()],
                context: BTreeMap::from([
                    ("tenant".to_string(), tenant.to_string()),
                    ("unit".to_string(), "u9".to_string()),
                ]),
                ..Default::default()
            })
            .unwrap()
        };
        let alice = owner("t1");
        let bob = owner("t2");

        // each tenant reads only its own scope key
        assert!(alice.check_scope("profile", "t1/u9/uid").is_ok());
        assert!(
            alice.check_scope("profile", "t2/u9/uid").is_err(),
            "another tenant's scope must be denied"
        );
        assert!(alice.check_rn("orders:t1:flow").is_ok());
        assert!(alice.check_rn("orders:t2:flow").is_err());
        assert!(bob.check_scope("profile", "t2/u9/uid").is_ok());
        assert!(bob.check_scope("profile", "t1/u9/uid").is_err());

        // the sealed policy carries the vocabulary, so subflows and the
        // seal-time re-check substitute against the starter's context
        let sealed = alice.scope_policy();
        assert_eq!(sealed.context.get("tenant").map(String::as_str), Some("t1"));
        assert!(sealed.allows("profile", "t1/u9/x"));
        assert!(!sealed.allows("profile", "t2/u9/x"));

        // whoami answers the vocabulary back to its own caller
        let who = alice.to_value();
        assert_eq!(who["context"]["tenant"], json!("t1"));
        assert_eq!(who["context"]["unit"], json!("u9"));
    }

    #[test]
    fn a_pattern_naming_an_absent_context_key_matches_nothing() {
        let policy = Principal::from_policy(&UserPolicy {
            name: "op".to_string(),
            enabled: true,
            snapshot: HashMap::from([(
                "profile".to_string(),
                vec!["$ctx.department/*".to_string()],
            )]),
            // the policy carries no `department`
            context: BTreeMap::from([("tenant".to_string(), "t1".to_string())]),
            ..Default::default()
        })
        .unwrap();
        assert!(
            policy.check_scope("profile", "d3").is_err(),
            "an absent $ctx key must fail closed, never widen"
        );
        assert!(!policy.scope_policy().allows("profile", ""));
    }

    #[test]
    fn context_validation_refuses_bad_keys_and_values() {
        let ok = BTreeMap::from([("tenant".to_string(), "t1".to_string())]);
        validate_context(&ok).unwrap();
        validate_context(&BTreeMap::new()).unwrap();

        for bad_key in ["", "Tenant", "1tenant", "ten-ant", "a".repeat(33).as_str()] {
            let err = validate_context(&BTreeMap::from([(bad_key.to_string(), "v".to_string())]))
                .unwrap_err();
            assert!(err.to_string().contains("invalid"), "{err}");
        }
        // 32 bytes is exactly the key limit
        validate_context(&BTreeMap::from([("a".repeat(32), "v".to_string())])).unwrap();

        // values: empty and oversized are refused, 128 bytes is the limit
        let err =
            validate_context(&BTreeMap::from([("tenant".to_string(), String::new())])).unwrap_err();
        assert!(err.to_string().contains("invalid value"), "{err}");
        let err = validate_context(&BTreeMap::from([("tenant".to_string(), "x".repeat(129))]))
            .unwrap_err();
        assert!(err.to_string().contains("invalid value"), "{err}");
        validate_context(&BTreeMap::from([("tenant".to_string(), "x".repeat(128))])).unwrap();

        // at most 16 entries
        let too_many: BTreeMap<String, String> = (0..17)
            .map(|i| (format!("k{i}"), "v".to_string()))
            .collect();
        let err = validate_context(&too_many).unwrap_err();
        assert!(err.to_string().contains("at most 16"), "{err}");
        let sixteen: BTreeMap<String, String> = (0..16)
            .map(|i| (format!("k{i}"), "v".to_string()))
            .collect();
        validate_context(&sixteen).unwrap();
    }

    #[test]
    fn a_policy_with_an_invalid_context_is_refused_at_compile() {
        let err = Principal::from_policy(&UserPolicy {
            name: "op".to_string(),
            enabled: true,
            context: BTreeMap::from([("Bad-Key".to_string(), "v".to_string())]),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("invalid"), "{err}");
    }

    #[test]
    fn a_userspec_carries_context_through_serde() {
        let spec: UserSpec = serde_json::from_value(json!({
            "name": "alice",
            "context": { "tenant": "t1", "unit": "u9" }
        }))
        .unwrap();
        assert_eq!(
            spec.context
                .as_ref()
                .unwrap()
                .get("tenant")
                .map(String::as_str),
            Some("t1")
        );
        // absent on the wire = unchanged
        let spec: UserSpec = serde_json::from_value(json!({ "name": "alice" })).unwrap();
        assert!(spec.context.is_none());
    }
}
