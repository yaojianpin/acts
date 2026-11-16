//! Store-backed users, passwords and login sessions for the acts workflow
//! engine.
//!
//! This crate is the engine's [`AccessControl`](acts::AccessControl)
//! implementation: it keeps a user registry and a session registry in the
//! engine's own store (the same database the workflow rows live in), verifies
//! passwords, mints and rotates session tokens, and answers the engine's ACL
//! questions with the [`Principal`](acts::Principal) a user's grants compile
//! to.
//!
//! An engine has no users until one installs this — a bare
//! `Engine::builder().start()` runs `acts::AnonymousAcl`, which knows nobody.
//! One call changes that:
//!
//! ```no_run
//! use acts::Engine;
//! use acts_acl::AclUsers;
//!
//! # async fn run() -> acts::Result<()> {
//! let engine = Engine::builder().with_user_acl().start().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## The registry
//!
//! Two collections live under their own prefixes in the engine's store:
//! `acl_users` (the rows an operator writes with `acl:setuser`) and
//! `acl_sessions` (one row per live login). Nothing else in the engine reads
//! them, so the schema travels with this crate.
//!
//! Every engine that installs it starts with the builtin [`ADMIN_USER`]:
//! unrestricted. On a fresh store its password comes from [`ADMIN_PASSWORD_ENV`]
//! or a dev bootstrap file; with neither, the registry opens the *bootstrap
//! window* — the engine serves, but nothing is allowed until a loopback caller
//! gives the builtin admin its first password with `acl:setuser`. The admin
//! cannot be deleted or disabled, so an engine can never lock itself out of
//! its own registry.
//!
//! ## Sessions
//!
//! `acl:login` answers a short-lived access token and a long-lived refresh
//! token. Only their sha256 digests are stored; a refresh rotates the pair and
//! kills the old one (a refresh token is single-use). Sessions are rows, so a
//! server restart does not log every caller out.

#![doc = include_str!("../README.md")]

use acts::query::{Expr, Filter, FilterExpr, FilterType, Query};
use acts::{
    AccessControl, AclError, ActError, DbCollectionIden, LoginTokens, Principal, Result, Store,
    UserPolicy, UserSpec,
};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::sync::Semaphore;

/// The builtin administrator every engine starts with: unrestricted, with a
/// generated password (or [`ADMIN_PASSWORD_ENV`]). It cannot be deleted or
/// disabled, so an engine can never lock itself out of its own ACL.
pub const ADMIN_USER: &str = "admin";

/// Environment variable that sets the builtin admin password at first start
/// (when the `acl_users` collection is still empty).
pub const ADMIN_PASSWORD_ENV: &str = "ACTS_ADMIN_PASSWORD";

/// The builtin admin's default password on a fresh store: the account is
/// created in the initial state and refuses every action except changing
/// this password (and logging out) until it does.
pub const DEFAULT_ADMIN_PASSWORD: &str = "admin";

/// How long an access token answers, in seconds.
pub const ACCESS_TOKEN_TTL_SECS: i64 = 3600;
/// How long a refresh token may rotate a session, in seconds.
pub const REFRESH_TOKEN_TTL_SECS: i64 = 7 * 24 * 3600;

const USERS_PREFIX: &str = "acl_users";
const SESSIONS_PREFIX: &str = "acl_sessions";

fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

/// Runtime knobs, read once from the environment.
struct AclConfig {
    /// Argon2id memory cost, KiB.
    argon2_m: u32,
    /// Argon2id time cost, iterations.
    argon2_t: u32,
    /// Argon2id parallelism.
    argon2_p: u32,
    /// How many password verifications may run at once (the KDF budget).
    kdf_permits: usize,
    /// Failed logins per user name before the exponential backoff starts.
    login_max_failures: u32,
    /// Backoff ceiling, seconds.
    login_backoff_max_secs: i64,
    /// Stored credentials a user may hold at once.
    max_passwords: usize,
    /// Active sessions a user holds by default (`max_sessions` overrides).
    default_max_sessions: usize,
    /// How long a cached session row trusts itself, millis. Past it the store
    /// re-reads — the multi-instance convergence bound.
    session_ttl_ms: i64,
    /// How long a cached user header trusts itself, millis.
    user_ttl_ms: i64,
    /// Seconds between the session GC passes.
    gc_interval_secs: u64,
}

fn env_num<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

static ACL_CONFIG: LazyLock<AclConfig> = LazyLock::new(|| AclConfig {
    // the OWASP starting point; a budgeted deployment raises these (see the
    // crate README) and the parameters travel inside every PHC string, so
    // existing hashes keep verifying
    argon2_m: env_num("ACTS_ARGON2_M_KIB", 19_456),
    argon2_t: env_num("ACTS_ARGON2_T", 2),
    argon2_p: env_num("ACTS_ARGON2_P", 1),
    kdf_permits: env_num("ACTS_ACL_KDF_CONCURRENCY", 8).max(1),
    login_max_failures: env_num("ACTS_ACL_LOGIN_MAX_FAILURES", 5).max(1),
    login_backoff_max_secs: env_num("ACTS_ACL_LOGIN_BACKOFF_MAX_SECS", 900),
    max_passwords: env_num("ACTS_ACL_MAX_PASSWORDS", 3).max(1),
    default_max_sessions: env_num("ACTS_ACL_MAX_SESSIONS", 10).max(1),
    session_ttl_ms: env_num("ACTS_ACL_SESSION_TTL_MS", 5_000i64).max(0),
    user_ttl_ms: env_num("ACTS_ACL_HEADER_TTL_MS", 2_000i64).max(0),
    gc_interval_secs: env_num("ACTS_ACL_GC_INTERVAL_SECS", 60u64).max(5),
});

/// The bootstrap password from the environment; test builds ignore the host's
/// export so the bootstrap path is deterministic.
fn admin_password_from_env() -> Option<String> {
    #[cfg(test)]
    if IGNORE_ADMIN_ENV_FOR_TESTS.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    std::env::var(ADMIN_PASSWORD_ENV)
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

/// Test-only: pretend the host exports no `ACTS_ADMIN_PASSWORD`.
#[cfg(test)]
static IGNORE_ADMIN_ENV_FOR_TESTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

fn acl_config() -> &'static AclConfig {
    &ACL_CONFIG
}

static ARGON2: LazyLock<Argon2<'static>> = LazyLock::new(|| {
    let cfg = acl_config();
    match Params::new(cfg.argon2_m, cfg.argon2_t, cfg.argon2_p, None) {
        // v0x13 — what the PHC header spells `v=19`
        Ok(params) => Argon2::new(Algorithm::Argon2id, Version::V0x13, params),
        Err(_) => Argon2::default(),
    }
});

/// `password` as an Argon2id PHC string with a fresh random salt.
fn phc_encode(password: &str) -> Result<String> {
    // 32 hex chars = a 128-bit salt, from the same OS source as the tokens
    let salt = SaltString::encode_b64(random_id(32).as_bytes()).expect("salt fits b64");
    ARGON2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| ActError::Store(format!("password hashing failed: {err}")))
}

/// Whether `password` verifies a stored PHC string. Entries in any other
/// format (rows written by an older binary) verify nothing.
fn phc_verify(entry: &str, password: &str) -> bool {
    let Ok(hash) = PasswordHash::new(entry) else {
        return false;
    };
    // the verifier reads the parameters from the PHC string itself
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

/// Whether `password` verifies any stored credential.
fn verify_any(passwords: &[String], password: &str) -> bool {
    passwords.iter().any(|entry| phc_verify(entry, password))
}

/// A fixed input for the dummy verification an unknown user runs: the PHC is
/// public knowledge — only the work it costs (and thus the response timing)
/// matters.
const DUMMY_PASSWORD: &str = "acts:bootstrap:dummy:Wv3Rc8ThQm";

static DUMMY_PHC: LazyLock<String> = LazyLock::new(|| {
    // a fixed salt keeps the dummy deterministic; its entropy is irrelevant
    let salt = SaltString::encode_b64(b"acts-dummy-salt!!").expect("static dummy salt");
    ARGON2
        .hash_password(DUMMY_PASSWORD.as_bytes(), &salt)
        .expect("static dummy hash")
        .to_string()
});

/// An unknown user burns the same KDF work as a known one.
fn dummy_verify(password: &str) {
    let _ = phc_verify(&DUMMY_PHC, password);
}

/// No raw password reaches the KDF above this size.
const MAX_PASSWORD_BYTES: usize = 1024;

/// One answer for "no such user", "off" and "wrong password" — and for every
/// budget refusal: the error must not tell an attacker which half it got.
const INVALID_CREDENTIALS: &str = "invalid user or password";

/// Login budget: per-name failure backoff and a per-name attempt bucket.
/// Both maps are process-local (each instance counts its own callers) and
/// bounded — past [`BUDGET_CAP`] names the map resets, so abuse protection
/// never becomes unbounded memory.
#[derive(Default)]
struct LoginBudget {
    /// name → (failure count, blocked until, millis)
    failures: HashMap<String, (u32, i64)>,
    /// name → (tokens, last refill, millis)
    attempts: HashMap<String, (f64, i64)>,
}

const BUDGET_CAP: usize = 10_000;
/// The attempt bucket: 5 burst, refilled at 10 per minute.
const ATTEMPT_BURST: f64 = 5.0;
const ATTEMPTS_PER_SEC: f64 = 10.0 / 60.0;

impl LoginBudget {
    fn blocked(&self, name: &str, now: i64) -> bool {
        self.failures
            .get(name)
            .is_some_and(|(_, until)| *until > now)
    }

    fn record_failure(&mut self, name: &str, now: i64, cfg: &AclConfig) {
        if self.failures.len() > BUDGET_CAP {
            self.failures.clear();
        }
        let entry = self.failures.entry(name.to_string()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        if entry.0 >= cfg.login_max_failures {
            let step = (entry.0 - cfg.login_max_failures).min(16);
            let secs = 1i64
                .checked_shl(step)
                .unwrap_or(cfg.login_backoff_max_secs)
                .min(cfg.login_backoff_max_secs);
            entry.1 = now + secs * 1000;
        }
    }

    fn clear_failures(&mut self, name: &str) {
        self.failures.remove(name);
    }

    fn allow_attempt(&mut self, name: &str, now: i64) -> bool {
        if self.attempts.len() > BUDGET_CAP {
            self.attempts.clear();
        }
        let entry = self
            .attempts
            .entry(name.to_string())
            .or_insert((ATTEMPT_BURST, now));
        let elapsed = (now - entry.1).max(0) as f64 / 1000.0;
        entry.0 = ATTEMPT_BURST.min(entry.0 + elapsed * ATTEMPTS_PER_SEC);
        entry.1 = now;
        if entry.0 >= 1.0 {
            entry.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One access-control user, stored in the `acl_users` collection (schema 1).
#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct AclUser {
    /// username; the row key. Attribution goes through `subject_id`, so a
    /// deleted and re-created name is a new security subject — no old session
    /// or refresh token ever binds to it.
    pub id: String,
    /// the immutable subject id this generation of the name was minted with
    pub subject_id: String,
    /// an off user authenticates nothing, `acl:login` included
    pub enabled: bool,
    /// bumped on disable/enable, password changes and forced logout; a
    /// session snapshots it at issue time and refuses to authenticate on
    /// mismatch — the O(1) mechanism behind "this user must log out now"
    pub security_version: u64,
    /// bumped on every successful guarded write; the optimistic-concurrency
    /// and principal-cache key
    pub document_revision: u64,
    /// Argon2id PHC password strings (`$argon2id$…`); an empty list means
    /// the user has no usable credential
    pub passwords: Vec<String>,
    /// active sessions this user may hold at once (`None` = the default cap)
    pub max_sessions: Option<u32>,
    /// the initial-state flag: while set, the account answered the builtin
    /// default password and every action but the password change is refused
    /// (and logout); the change clears it and the security bump forces a
    /// fresh login
    #[serde(default)]
    pub must_change_password: bool,
    /// command/catalog patterns the user may run; `*`/`@all` = unrestricted.
    /// A token starting with `@` names a catalog group (`@read`, `@write`,
    /// `@deploy`, `@execute`) instead of a command glob.
    pub allow: Vec<String>,
    /// command/catalog patterns the user must never run; wins over `allow`
    pub deny: Vec<String>,
    /// resource-name (`rn`) patterns: which workflow resources this user may
    /// deploy and run
    pub patterns: Vec<String>,
    /// per snapshot target, the scope patterns the user owns
    /// (json `HashMap<String, Vec<String>>`; `$subject` allowed)
    pub snapshot: String,

    pub create_time: i64,
    pub update_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl AclUser {
    /// The user's grants as the engine's policy language sees them.
    pub fn policy(&self) -> Result<UserPolicy> {
        Ok(UserPolicy {
            name: self.id.clone(),
            enabled: self.enabled,
            allow: self.allow.clone(),
            deny: self.deny.clone(),
            patterns: self.patterns.clone(),
            snapshot: if self.snapshot.trim().is_empty() {
                HashMap::new()
            } else {
                serde_json::from_str(&self.snapshot).map_err(|err| {
                    ActError::Store(format!(
                        "acl user '{}' has an invalid snapshot rule: {err}",
                        self.id
                    ))
                })?
            },
        })
    }
}

impl DbCollectionIden for AclUser {
    fn iden() -> String {
        USERS_PREFIX.to_string()
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["timestamp", "create_time", "update_time"]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
        &["timestamp", "create_time", "update_time"]
    }

    fn version() -> i32 {
        1
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(ActError::Store(format!(
            "unsupported acl user version: {v} — this build reads schema {} only; \
             an older 0.x store has to be reset (clear the '{USERS_PREFIX}' collection \
             or the data directory) before this engine starts",
            Self::version()
        )))
    }
}

/// The lifecycle of one login session, stored inside its `acl_sessions` row
/// (internally tagged as the `state` field of the stored JSON).
///
/// Only `Active` authenticates anything. `Consumed` is the single-use proof a
/// refresh leaves behind — a presented refresh token whose session is
/// `Consumed` (or `Revoked`) is a replay, and revokes the whole family.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state")]
pub enum SessionState {
    Active,
    Consumed {
        at: i64,
        /// the `session_id` of the successor this rotation minted
        successor: String,
    },
    Revoked {
        at: i64,
        /// logout / replay / expired / quota_evicted
        reason: String,
    },
}

/// One login session, stored in the `acl_sessions` collection (schema 1).
///
/// The tokens themselves are never stored — only their sha256 digests. The
/// access digest is the row id; the refresh digest is an indexed field, and
/// `session_id`/`family_id` link a rotation to its login generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclSession {
    /// sha256(access token); the row key
    pub id: String,
    /// the identity of this session, minted once and never rotated
    pub session_id: String,
    /// the login generation's first `session_id`; every rotation inherits it
    pub family_id: String,
    /// the `session_id` this session was rotated from (`None` on fresh login)
    pub parent_session_id: Option<String>,
    /// username the pair authenticates as
    pub user: String,
    /// the user's `subject_id` at issue time — a re-created name never
    /// matches, so an orphaned row can never authenticate again
    pub subject_id: String,
    /// the user's `security_version` at issue time
    pub security_version: u64,
    /// sha256(refresh token)
    pub refresh: String,
    /// access-token expiry, millis since epoch
    pub access_expiry: i64,
    /// refresh-token expiry, millis since epoch
    pub refresh_expiry: i64,
    pub state: SessionState,
    /// bumped on every guarded state write (diagnostics)
    pub revision: u64,

    pub create_time: i64,
    pub update_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for AclSession {
    fn iden() -> String {
        SESSIONS_PREFIX.to_string()
    }

    fn indexed_fields() -> &'static [&'static str] {
        // `state` is deliberately not indexed: the internally tagged enum
        // serializes as a nested object, which no scalar filter can match —
        // callers that care filter in memory after a `user`/`family_id` scan
        &[
            "timestamp",
            "create_time",
            "update_time",
            "refresh",
            "user",
            "family_id",
        ]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
        &["timestamp", "create_time", "update_time", "refresh_expiry"]
    }

    fn version() -> i32 {
        1
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(ActError::Store(format!(
            "unsupported acl session version: {v} — this build reads schema {} only; \
             an older 0.x store has to be reset (clear the '{SESSIONS_PREFIX}' collection \
             or the data directory) before this engine starts",
            Self::version()
        )))
    }
}

/// The record a deleted user leaves behind: which subject generations a name
/// had, when the last one went away, and at what security version. No policy
/// and no credential ever lives here — it exists for audit attribution and
/// same-name re-creation history.
#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct AclTombstone {
    /// the deleted generation's `subject_id`; the row key
    pub id: String,
    /// the name the generation answered to
    pub name: String,
    pub last_security_version: u64,
    pub deleted_at: i64,
    pub v: i32,
}

impl DbCollectionIden for AclTombstone {
    fn iden() -> String {
        "acl_tombstones".to_string()
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["name"]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
        &["deleted_at"]
    }

    fn version() -> i32 {
        1
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(ActError::Store(format!(
            "unsupported acl tombstone version: {v}"
        )))
    }
}

/// One user with its grants already compiled into a principal.
#[derive(Debug, Clone)]
struct CompiledUser {
    row: AclUser,
    principal: Principal,
}

/// The store-backed user registry and session store.
///
/// Constructed before the engine knows its store (`UserAcl::new`), bound to
/// it when the engine starts.
pub struct UserAcl {
    store: RwLock<Option<Arc<Store>>>,
    /// name -> (compiled header, fetched at): the header cache, TTL-bounded
    users: RwLock<HashMap<String, (Arc<CompiledUser>, i64)>>,
    /// access-token hash -> (session row, fetched at): write-through, TTL-bounded
    sessions: RwLock<HashMap<String, (AclSession, i64)>>,
    /// refresh-token hash -> access-token hash
    refresh: RwLock<HashMap<String, String>>,
    /// The KDF budget: how many password verifications run at once.
    kdf_permits: Arc<Semaphore>,
    /// Per-name login budget (failure backoff + attempt bucket).
    budget: Mutex<LoginBudget>,
}

/// One GC loop per process, whatever the embedder constructs.
static GC_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl std::fmt::Debug for UserAcl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the store handle has no Debug of its own, and the counts are what a
        // log line wants anyway
        f.debug_struct("UserAcl")
            .field("bound", &self.store.read().is_some())
            .field("users", &self.users.read().len())
            .field("sessions", &self.sessions.read().len())
            .finish()
    }
}

impl Default for UserAcl {
    fn default() -> Self {
        Self::new()
    }
}

impl UserAcl {
    /// An empty registry, to be bound to the engine's store at start.
    ///
    /// On a fresh store the builtin admin is created with the default
    /// password ([`DEFAULT_ADMIN_PASSWORD`]) and the initial-state flag: it
    /// can log in, but every action except changing that password (and
    /// logging out) is refused until it does, and the change forces a fresh
    /// login. `ACTS_ADMIN_PASSWORD` skips the default entirely.
    pub fn new() -> Self {
        Self {
            store: RwLock::new(None),
            users: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            refresh: RwLock::new(HashMap::new()),
            kdf_permits: Arc::new(Semaphore::new(acl_config().kdf_permits)),
            budget: Mutex::new(LoginBudget::default()),
        }
    }

    /// The users currently loaded, by name.
    fn store(&self) -> Option<Arc<Store>> {
        self.store.read().clone()
    }

    /// Whether the per-name failure backoff currently blocks this user.
    #[cfg(test)]
    fn budget_blocked(&self, name: &str) -> bool {
        self.budget.lock().blocked(name, now_millis())
    }

    /// Mint a fresh token pair and persist the session row before anything is
    /// answered. `family_id`/`parent_session_id` are `None` for a fresh login
    /// (the session starts its own family) and set on a refresh rotation.
    async fn mint(
        &self,
        user: &CompiledUser,
        family_id: Option<String>,
        parent_session_id: Option<String>,
    ) -> std::result::Result<LoginTokens, AclError> {
        let now = now_millis();
        let token = format!("at_{}", random_id(32));
        let refresh_token = format!("rt_{}", random_id(32));
        let access_hash = sha256_hex(&token);
        let refresh_hash = sha256_hex(&refresh_token);
        let session_id = random_id(16);
        let session = AclSession {
            id: access_hash.clone(),
            session_id: session_id.clone(),
            family_id: family_id.unwrap_or_else(|| session_id.clone()),
            parent_session_id,
            user: user.row.id.clone(),
            subject_id: user.row.subject_id.clone(),
            security_version: user.row.security_version,
            refresh: refresh_hash.clone(),
            access_expiry: now + ACCESS_TOKEN_TTL_SECS * 1000,
            refresh_expiry: now + REFRESH_TOKEN_TTL_SECS * 1000,
            state: SessionState::Active,
            revision: 1,
            create_time: now,
            update_time: now,
            timestamp: now,
            v: AclSession::version(),
        };

        if let Some(store) = self.store()
            && let Err(err) = store.collection::<AclSession>().create(&session).await
        {
            return Err(AclError::Denied(format!(
                "failed to persist the session: {err}"
            )));
        }
        self.sessions
            .write()
            .insert(access_hash.clone(), (session, now_millis()));
        self.refresh.write().insert(refresh_hash, access_hash);

        Ok(LoginTokens {
            token,
            refresh_token,
            expires_in: ACCESS_TOKEN_TTL_SECS,
            refresh_expires_in: REFRESH_TOKEN_TTL_SECS,
        })
    }

    /// The principal a live session resolves to, if the session is `Active`,
    /// unexpired, and still matches its user's live subject and security
    /// version. This is the legacy, cache-only path behind
    /// [`AccessControl::authenticate`]; `resolve` adds the store fallback.
    fn session_principal(&self, token: &str) -> Option<Principal> {
        let hash = sha256_hex(token);
        let session = {
            let sessions = self.sessions.read();
            sessions.get(&hash)?.0.clone()
        };
        let user = {
            let users = self.users.read();
            users.get(&session.user)?.0.clone()
        };
        self.live_principal(&session, &user)
    }

    /// Whether `session` may authenticate as its user right now, and as whom.
    fn live_principal(&self, session: &AclSession, user: &CompiledUser) -> Option<Principal> {
        if !matches!(session.state, SessionState::Active) {
            return None;
        }
        if session.access_expiry < now_millis() {
            return None;
        }
        // the generation check: a deleted and re-created name mints a new
        // subject_id, and a security bump invalidates the sessions issued
        // before it — neither is a match, so neither authenticates
        if !user.row.enabled
            || user.row.subject_id != session.subject_id
            || user.row.security_version != session.security_version
        {
            return None;
        }
        Some(user.principal.clone())
    }

    /// The user header behind a name, TTL-fresh: a hit inside the header TTL
    /// trusts the cache; past it the store re-reads — and a store failure
    /// fails closed. A stale header never outlives its TTL into a grant, and
    /// another instance's disable/delete lands within one TTL.
    async fn user_of(&self, name: &str) -> Option<Arc<CompiledUser>> {
        let now = now_millis();
        let cached = {
            let users = self.users.read();
            users.get(name).cloned()
        };
        if let Some((user, fetched_at)) = cached
            && now - fetched_at <= acl_config().user_ttl_ms
        {
            return Some(user);
        }
        let store = self.store()?;
        match store.collection::<AclUser>().find_opt(name).await {
            Ok(Some(row)) => {
                let compiled = match row
                    .policy()
                    .and_then(|policy| Principal::from_policy(&policy))
                {
                    Ok(principal) => Arc::new(CompiledUser { row, principal }),
                    Err(err) => {
                        tracing::warn!(
                            user = %name, error = %err,
                            "acl user row failed to compile; failing closed"
                        );
                        return None;
                    }
                };
                self.users
                    .write()
                    .insert(name.to_string(), (Arc::clone(&compiled), now));
                Some(compiled)
            }
            Ok(None) => {
                // gone on another instance: drop the stale header so a later
                // re-creation of the name reads fresh
                self.users.write().remove(name);
                None
            }
            Err(err) => {
                tracing::warn!(
                    user = %name, error = %err,
                    "acl store read failed: failing closed"
                );
                None
            }
        }
    }

    /// The session row behind a token: the local write-through cache inside
    /// its TTL, the store otherwise — and a store failure fails closed
    /// (an unknown answer, never a silent pass).
    async fn session_row(&self, access_hash: &str) -> Option<AclSession> {
        let now = now_millis();
        let cached = self.sessions.read().get(access_hash).cloned();
        if let Some((session, fetched_at)) = cached {
            if now - fetched_at <= acl_config().session_ttl_ms {
                return Some(session);
            }
            self.sessions.write().remove(access_hash);
        }
        let store = self.store()?;
        match store.collection::<AclSession>().find_opt(access_hash).await {
            Ok(Some(row)) => {
                self.sessions
                    .write()
                    .insert(access_hash.to_string(), (row.clone(), now));
                Some(row)
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err, "acl store read failed: failing closed");
                None
            }
        }
    }

    /// Locate a session by its refresh-token hash: the local index first, the
    /// `refresh` equality index second.
    async fn session_by_refresh(&self, refresh_hash: &str) -> Option<AclSession> {
        let access_hash = self.refresh.read().get(refresh_hash).cloned();
        if let Some(access_hash) = access_hash
            && let Some((session, _)) = self.sessions.read().get(&access_hash)
        {
            return Some(session.clone());
        }
        let store = self.store()?;
        let query = Query {
            offset: 0,
            limit: 100,
            filter: Some(Filter {
                r#type: FilterType::And,
                exprs: vec![FilterExpr::Expr(Expr::eq("refresh", refresh_hash))],
            }),
            order_by: Vec::new(),
        };
        store
            .collection::<AclSession>()
            .query(&query)
            .await
            .ok()?
            .rows
            .into_iter()
            .find(|row| row.refresh == refresh_hash)
    }

    /// One presentation too many: revoke the family (CAS per row — the goal
    /// is a dead family, not a tidy one) and answer the same words an
    /// unknown token gets.
    async fn replay(&self, family_id: &str, keep: &str) -> AclError {
        tracing::warn!(family = %family_id, "refresh replay detected: revoking the family");
        let mut revoked = 0usize;
        if let Some(store) = self.store() {
            let query = Query {
                offset: 0,
                limit: 100,
                filter: Some(Filter {
                    r#type: FilterType::And,
                    exprs: vec![FilterExpr::Expr(Expr::eq("family_id", family_id))],
                }),
                order_by: Vec::new(),
            };
            let rows = match store.collection::<AclSession>().query(&query).await {
                Ok(page) => page.rows,
                Err(err) => {
                    tracing::warn!(error = %err, "replay family scan failed");
                    return self.replay_error();
                }
            };
            let now = now_millis();
            for row in rows {
                if row.id == keep || !matches!(row.state, SessionState::Active) {
                    continue;
                }
                let mut dead = row.clone();
                dead.state = SessionState::Revoked {
                    at: now,
                    reason: "replay".to_string(),
                };
                dead.revision += 1;
                dead.update_time = now;
                dead.timestamp = now;
                if store
                    .collection::<AclSession>()
                    .update_guarded(&dead, &row)
                    .await
                    .unwrap_or(false)
                {
                    self.sessions.write().remove(&row.id);
                    self.refresh.write().remove(&row.refresh);
                    revoked += 1;
                }
            }
        }
        if revoked == 0 {
            tracing::debug!(family = %family_id, "replay family scan found nothing live");
        }
        self.replay_error()
    }

    fn replay_error(&self) -> AclError {
        AclError::Unauthenticated("the refresh token is unknown or was already used".to_string())
    }

    /// Evict the oldest active sessions past the user's cap, best effort: a
    /// lost eviction or a failed scan still lets the login through (the cap
    /// is a budget, not a security boundary), and concurrent logins may
    /// overshoot by the design-accepted epsilon.
    async fn enforce_quota(&self, user: &CompiledUser) {
        let cap = user
            .row
            .max_sessions
            .map(|value| value as usize)
            .unwrap_or(acl_config().default_max_sessions);
        let Some(store) = self.store() else {
            return;
        };
        let query = Query {
            offset: 0,
            limit: (cap + 1).clamp(1, 100),
            filter: Some(Filter {
                r#type: FilterType::And,
                exprs: vec![FilterExpr::Expr(Expr::eq("user", user.row.id.clone()))],
            }),
            order_by: Vec::new(),
        };
        let rows = match store.collection::<AclSession>().query(&query).await {
            Ok(page) => page.rows,
            Err(err) => {
                tracing::warn!(error = %err, "session quota scan failed");
                return;
            }
        };
        let mut live: Vec<AclSession> = rows
            .into_iter()
            .filter(|row| matches!(row.state, SessionState::Active))
            .collect();
        if live.len() < cap {
            return;
        }
        live.sort_by_key(|row| row.create_time);
        let evict = live.len().saturating_sub(cap - 1);
        let now = now_millis();
        for victim in live.into_iter().take(evict) {
            let mut dead = victim.clone();
            dead.state = SessionState::Revoked {
                at: now,
                reason: "quota_evicted".to_string(),
            };
            dead.revision += 1;
            dead.update_time = now;
            dead.timestamp = now;
            if store
                .collection::<AclSession>()
                .update_guarded(&dead, &victim)
                .await
                .unwrap_or(false)
            {
                self.sessions.write().remove(&victim.id);
                self.refresh.write().remove(&victim.refresh);
                tracing::info!(
                    user = %user.row.id,
                    session = %victim.session_id,
                    "session evicted by the quota"
                );
            }
        }
    }

    /// One GC pass: page over the sessions whose refresh token expired,
    /// oldest first, and remove them (row + index rows). The delete is
    /// idempotent per id, so instances share the pass without coordination.
    /// Answers the deleted count; a failed scan keeps the backlog for the
    /// next pass.
    async fn gc_pass(store: &Arc<Store>) -> usize {
        let now = now_millis();
        let query = Query {
            offset: 0,
            limit: 500,
            filter: Some(Filter {
                r#type: FilterType::And,
                exprs: vec![FilterExpr::Expr(Expr {
                    key: "refresh_expiry".to_string(),
                    value: json!(now),
                    op: acts::query::ExprOp::LT,
                })],
            }),
            order_by: vec![acts::query::OrderBy {
                field: "refresh_expiry".to_string(),
                order: acts::query::Sort::Asc,
            }],
        };
        let rows = match store.collection::<AclSession>().query(&query).await {
            Ok(page) => page.rows,
            Err(err) => {
                tracing::warn!(error = %err, "acl gc scan failed");
                return 0;
            }
        };
        let col = store.collection::<AclSession>();
        let mut deleted = 0;
        for row in rows {
            if col.delete(&row.id).await.unwrap_or(false) {
                deleted += 1;
            }
        }
        if deleted > 0 {
            tracing::info!(deleted, "acl gc pass removed expired sessions");
        }
        deleted
    }

    /// Spawn the periodic GC loop — one per process, owning only the store
    /// handle (local session caches are TTL-bounded, so they age out on their
    /// own; no instance holds a loop hostage to another's registry).
    fn start_gc(&self, store: Arc<Store>) {
        if GC_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let interval = std::time::Duration::from_secs(acl_config().gc_interval_secs);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                Self::gc_pass(&store).await;
            }
        });
    }
}
#[async_trait::async_trait]
impl AccessControl for UserAcl {
    /// Load the users and live sessions from the store, and bootstrap the
    /// builtin admin when no user exists yet.
    async fn load(&self, store: Arc<Store>) -> Result<()> {
        *self.store.write() = Some(store.clone());

        let mut users = HashMap::new();
        for row in store
            .collection::<AclUser>()
            .query_all(&Query::new())
            .await?
        {
            let name = row.id.clone();
            let principal = Principal::from_policy(&row.policy()?)?;
            users.insert(
                name,
                (Arc::new(CompiledUser { row, principal }), now_millis()),
            );
        }

        let now = now_millis();
        let mut sessions = HashMap::new();
        let mut refresh = HashMap::new();
        for row in store
            .collection::<AclSession>()
            .query_all(&Query::new())
            .await?
        {
            // only live sessions materialize; consumed, revoked and expired
            // rows stay in the store for the GC pass (they are the audit
            // trail and the replay proof)
            if !matches!(row.state, SessionState::Active) || row.refresh_expiry < now {
                continue;
            }
            refresh.insert(row.refresh.clone(), row.id.clone());
            sessions.insert(row.id.clone(), (row, now_millis()));
        }

        // First start (or a store without users): the builtin admin. Its
        // password comes from `ACTS_ADMIN_PASSWORD`; without it the row is
        // born with the documented default password and the initial-state
        // flag — the account can log in, but the engine refuses every action
        // except changing that password (and logout) until it does, and the
        // change bumps the security version so the next request must log in
        // again (see the gate in `actions::apply_as`).
        if users.is_empty() {
            let col = store.collection::<AclUser>();
            let env_password = admin_password_from_env();
            let row = match env_password {
                Some(password) => {
                    let row = admin_row(&password)?;
                    tracing::info!(
                        user = ADMIN_USER,
                        "created the builtin '{ADMIN_USER}' from {ADMIN_PASSWORD_ENV}"
                    );
                    row
                }
                None => {
                    tracing::warn!(
                        user = ADMIN_USER,
                        "the builtin '{ADMIN_USER}' was created with the default password \
                         '{DEFAULT_ADMIN_PASSWORD}': every action is refused until it is \
                         changed (acl:setuser, then acl:login again); set \
                         {ADMIN_PASSWORD_ENV} to skip the default"
                    );
                    admin_row_default()?
                }
            };
            if col.create(&row).await? {
                let principal = Principal::from_policy(&row.policy()?)?;
                users.insert(
                    row.id.clone(),
                    (Arc::new(CompiledUser { row, principal }), now_millis()),
                );
            } else {
                // another instance won the first-start race: serve its row
                let existing = col.find(ADMIN_USER).await?;
                let principal = Principal::from_policy(&existing.policy()?)?;
                users.insert(
                    existing.id.clone(),
                    (
                        Arc::new(CompiledUser {
                            row: existing,
                            principal,
                        }),
                        now_millis(),
                    ),
                );
            }
        }
        *self.users.write() = users;
        *self.sessions.write() = sessions;
        *self.refresh.write() = refresh;
        self.start_gc(store);
        Ok(())
    }

    fn enabled(&self) -> bool {
        true
    }

    fn password_change_required(&self, subject: &str) -> bool {
        self.users
            .read()
            .get(subject)
            .is_some_and(|(user, _)| user.row.must_change_password)
    }

    fn authenticate(&self, token: Option<&str>) -> std::result::Result<Principal, AclError> {
        if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty())
            && let Some(principal) = self.session_principal(token)
        {
            return Ok(principal);
        }
        Ok(Principal::anonymous())
    }

    /// The precise three-state resolution: an unknown, expired, consumed,
    /// revoked or generation-mismatched token is [`AuthenticationResult::
    /// InvalidCredentials`] — never folded into the anonymous caller.
    async fn resolve(&self, token: Option<&str>) -> acts::AuthenticationResult {
        let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
            return acts::AuthenticationResult::MissingCredentials;
        };
        let hash = sha256_hex(token);
        let Some(session) = self.session_row(&hash).await else {
            return acts::AuthenticationResult::InvalidCredentials;
        };
        let Some(user) = self.user_of(&session.user).await else {
            return acts::AuthenticationResult::InvalidCredentials;
        };
        match self.live_principal(&session, &user) {
            Some(principal) => acts::AuthenticationResult::Authenticated(principal),
            None => acts::AuthenticationResult::InvalidCredentials,
        }
    }

    async fn login(
        &self,
        user: &str,
        password: &str,
    ) -> std::result::Result<LoginTokens, AclError> {
        let name = user.trim();
        // 1. the input bound: no KDF work for an oversized password
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(AclError::Unauthenticated(INVALID_CREDENTIALS.to_string()));
        }
        // 2. the per-name budget: backoff first, then the attempt bucket —
        //    a refused attempt looks exactly like a wrong password
        let now = now_millis();
        {
            let mut budget = self.budget.lock();
            if budget.blocked(name, now) || !budget.allow_attempt(name, now) {
                tracing::warn!(user = %name, "acts_acl_login_rate_limited_total");
                return Err(AclError::Unauthenticated(INVALID_CREDENTIALS.to_string()));
            }
        }
        let compiled = self.user_of(name).await;
        // 3. the KDF budget: at most `kdf_permits` verifications at once, and
        //    an unknown user burns the same dummy work as a known one
        let permit = self.kdf_permits.clone().acquire_owned().await;
        let compiled = match compiled {
            None => {
                dummy_verify(password);
                Err(AclError::Unauthenticated(INVALID_CREDENTIALS.to_string()))
            }
            Some(compiled) => {
                if !compiled.row.enabled || !verify_any(&compiled.row.passwords, password) {
                    self.budget
                        .lock()
                        .record_failure(name, now_millis(), acl_config());
                    Err(AclError::Unauthenticated(INVALID_CREDENTIALS.to_string()))
                } else {
                    Ok(compiled)
                }
            }
        };
        drop(permit);
        let compiled = compiled?;
        self.budget.lock().clear_failures(name);
        self.enforce_quota(&compiled).await;
        self.mint(&compiled, None, None).await
    }

    async fn refresh(&self, refresh_token: &str) -> std::result::Result<LoginTokens, AclError> {
        let refresh_token = refresh_token.trim();
        if refresh_token.is_empty() {
            return Err(AclError::Unauthenticated(
                "a refresh token is required".to_string(),
            ));
        }
        let refresh_hash = sha256_hex(refresh_token);
        let old = self
            .session_by_refresh(&refresh_hash)
            .await
            .ok_or_else(|| self.replay_error())?;
        if old.refresh != refresh_hash {
            return Err(self.replay_error());
        }
        let now = now_millis();
        if old.refresh_expiry < now {
            // expiry is not a replay — mark the row and ask for a login
            if let Some(store) = self.store() {
                let mut expired = old.clone();
                expired.state = SessionState::Revoked {
                    at: now,
                    reason: "expired".to_string(),
                };
                expired.revision += 1;
                expired.update_time = now;
                expired.timestamp = now;
                let _ = store
                    .collection::<AclSession>()
                    .update_guarded(&expired, &old)
                    .await;
                self.sessions.write().remove(&old.id);
                self.refresh.write().remove(&old.refresh);
            }
            return Err(AclError::Unauthenticated(
                "the refresh token expired; login again".to_string(),
            ));
        }
        if !matches!(old.state, SessionState::Active) {
            // a consumed or revoked session presenting its refresh again is a
            // replay: the family dies
            tracing::warn!(family = %old.family_id, "acts_acl_refresh_replay_total");
            return Err(self.replay(&old.family_id, &old.id).await);
        }
        // the subject must still be the live user at the snapshotted version
        let Some(user) = self.user_of(&old.user).await else {
            return Err(AclError::Unauthenticated(INVALID_CREDENTIALS.to_string()));
        };
        if !user.row.enabled
            || user.row.subject_id != old.subject_id
            || user.row.security_version != old.security_version
        {
            return Err(AclError::Unauthenticated(INVALID_CREDENTIALS.to_string()));
        }

        // the rotation: consume the old row and mint the successor in ONE
        // guarded batch — either the family moves forward or it does not
        let tokens = LoginTokens {
            token: format!("at_{}", random_id(32)),
            refresh_token: format!("rt_{}", random_id(32)),
            expires_in: ACCESS_TOKEN_TTL_SECS,
            refresh_expires_in: REFRESH_TOKEN_TTL_SECS,
        };
        let access_hash = sha256_hex(&tokens.token);
        let refresh_hash = sha256_hex(&tokens.refresh_token);
        let session_id = random_id(16);
        let now = now_millis();
        let successor = AclSession {
            id: access_hash.clone(),
            session_id: session_id.clone(),
            family_id: old.family_id.clone(),
            parent_session_id: Some(old.session_id.clone()),
            user: user.row.id.clone(),
            subject_id: user.row.subject_id.clone(),
            security_version: user.row.security_version,
            refresh: refresh_hash,
            access_expiry: now + ACCESS_TOKEN_TTL_SECS * 1000,
            refresh_expiry: now + REFRESH_TOKEN_TTL_SECS * 1000,
            state: SessionState::Active,
            revision: 1,
            create_time: now,
            update_time: now,
            timestamp: now,
            v: AclSession::version(),
        };
        let mut consumed = old.clone();
        consumed.state = SessionState::Consumed {
            at: now,
            successor: session_id,
        };
        consumed.revision += 1;
        consumed.update_time = now;
        consumed.timestamp = now;

        if let Some(store) = self.store() {
            let col = store.collection::<AclSession>();
            // Consume the old row and create the successor as two guarded
            // steps: the consume CAS is the single-use decision exactly one
            // racer can win, and the successor carries a fresh id, so its
            // insert cannot collide. A failure between the two leaves the
            // caller logged out (re-login), never two live successors — the
            // safety invariant without a cross-document batch.
            if !col
                .update_guarded(&consumed, &old)
                .await
                .map_err(|err| AclError::Denied(format!("the refresh failed: {err}")))?
            {
                // someone else consumed this token first: a replay, or a
                // duplicate of an in-flight request — either way the family
                // dies and the caller logs in again
                return Err(self.replay(&old.family_id, &old.id).await);
            }
            if let Err(err) = col.create(&successor).await {
                return Err(AclError::Denied(format!(
                    "the refresh consumed its session but could not mint the successor: \
                     {err}; login again"
                )));
            }
        }
        self.sessions.write().remove(&old.id);
        {
            let mut refresh = self.refresh.write();
            refresh.remove(&old.refresh);
            refresh.insert(successor.refresh.clone(), access_hash.clone());
        }
        self.sessions
            .write()
            .insert(access_hash, (successor, now_millis()));

        Ok(tokens)
    }

    async fn logout(&self, token: &str) -> Result<bool> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(false);
        }
        let hash = sha256_hex(token);
        // the token may be the access token (the row id) or the refresh token
        // (an index into the same session)
        let access_hash = self.refresh.read().get(&hash).cloned().unwrap_or(hash);
        let Some(row) = self.session_row(&access_hash).await else {
            return Ok(false);
        };
        if !matches!(row.state, SessionState::Active) {
            // already out — idempotent, and never revived
            return Ok(false);
        }
        let now = now_millis();
        let mut revoked = row.clone();
        revoked.state = SessionState::Revoked {
            at: now,
            reason: "logout".to_string(),
        };
        revoked.revision += 1;
        revoked.update_time = now;
        revoked.timestamp = now;
        if let Some(store) = self.store() {
            // the CAS is the durable fact; a lost race means the session was
            // consumed or revoked meanwhile — it is out either way
            if !store
                .collection::<AclSession>()
                .update_guarded(&revoked, &row)
                .await?
            {
                self.sessions.write().remove(&row.id);
                self.refresh.write().remove(&row.refresh);
                return Ok(false);
            }
        }
        self.sessions.write().remove(&row.id);
        self.refresh.write().remove(&row.refresh);
        Ok(true)
    }

    async fn set_user(&self, spec: &UserSpec) -> Result<()> {
        let name = spec.name.trim().to_string();
        if name.is_empty() {
            return Err(ActError::Config(
                "acl user name cannot be empty".to_string(),
            ));
        }
        // The builtin admin cannot be turned off: an engine must always be
        // able to administer its own ACL.
        if name == ADMIN_USER && spec.enabled == Some(false) {
            return Err(ActError::Config(format!(
                "the builtin user '{ADMIN_USER}' cannot be disabled"
            )));
        }

        // the stored row is the only authority: a cache miss still reads the
        // store, so a user created on another instance is patched, not
        // re-created
        let store = self
            .store()
            .ok_or_else(|| ActError::Store("the acl is not bound to a store".to_string()))?;
        let col = store.collection::<AclUser>();
        let stored = col.find_opt(&name).await?;

        // compute the next row against the stored one
        let mut row = match &stored {
            Some(row) => row.clone(),
            None => AclUser {
                id: name.clone(),
                subject_id: random_id(16),
                enabled: true,
                security_version: 1,
                document_revision: 1,
                create_time: now_millis(),
                v: AclUser::version(),
                ..Default::default()
            },
        };
        if let Some(enabled) = spec.enabled {
            row.enabled = enabled;
        }
        if let Some(allow) = &spec.allow {
            row.allow = normalize_list(allow);
        }
        if let Some(deny) = &spec.deny {
            row.deny = normalize_list(deny);
        }
        if let Some(patterns) = &spec.patterns {
            row.patterns = normalize_list(patterns);
        }
        if let Some(snapshot) = &spec.snapshot {
            row.snapshot = serde_json::to_string(snapshot)
                .map_err(|err| ActError::Config(format!("invalid snapshot rule: {err}")))?;
        }
        if let Some(cap) = spec.max_sessions {
            row.max_sessions = Some(cap.max(1));
        }
        for password in spec
            .rm_passwords
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
        {
            // every stored credential that verifies this plaintext goes
            row.passwords.retain(|entry| !phc_verify(entry, password));
        }
        if !spec.add_passwords.is_empty() {
            let cfg = acl_config();
            if row.passwords.len() + spec.add_passwords.len() > cfg.max_passwords {
                return Err(ActError::Config(format!(
                    "a user may hold at most {} passwords",
                    cfg.max_passwords
                )));
            }
        }
        // The password change that ends the initial state replaces the
        // default credential: the old passwords go before the new one lands
        // (keeping the default password alive would defeat the flag), and the
        // security bump above forces the next request to log in again.
        if row.must_change_password && !spec.add_passwords.is_empty() {
            row.passwords.clear();
            row.must_change_password = false;
        }
        for password in spec.add_passwords.iter().map(|p| p.trim()) {
            if password.is_empty() {
                return Err(ActError::Config(
                    "acl user password cannot be empty".to_string(),
                ));
            }
            let hashed = phc_encode(password)?;
            row.passwords.push(hashed);
        }

        // the security version moves whenever the caller's relationship to
        // the credentials changes: disable/enable, password add/remove, or
        // an explicit forced logout
        if spec.enabled.is_some()
            || !spec.rm_passwords.is_empty()
            || !spec.add_passwords.is_empty()
            || spec.force_logout == Some(true)
        {
            row.security_version = row.security_version.saturating_add(1);
        }
        row.document_revision = row.document_revision.saturating_add(1);

        // compile first: a bad pattern must fail before anything is stored
        let policy = row.policy()?;
        let principal = Principal::from_policy(&policy)?;

        let now = now_millis();
        row.update_time = now;
        row.timestamp = now;
        match &stored {
            Some(expected) => {
                if !col.update_guarded(&row, expected).await? {
                    return Err(ActError::Conflict(format!(
                        "acl user '{name}' changed concurrently; re-read and retry"
                    )));
                }
            }
            None => {
                col.create(&row).await?;
            }
        }
        self.users.write().insert(
            name,
            (Arc::new(CompiledUser { row, principal }), now_millis()),
        );
        Ok(())
    }

    async fn del_user(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name == ADMIN_USER {
            return Err(ActError::Config(format!(
                "the builtin user '{ADMIN_USER}' cannot be deleted"
            )));
        }
        let store = self
            .store()
            .ok_or_else(|| ActError::Store("the acl is not bound to a store".to_string()))?;
        let col = store.collection::<AclUser>();
        let stored = col
            .find_opt(name)
            .await?
            .ok_or_else(|| ActError::Config(format!("acl user '{name}' does not exist")))?;

        // The tombstone records the generation first (history only — no
        // secret), then the row delete lands as the security fact: a session
        // of this user can never authenticate again, because its subject no
        // longer resolves. `set_user` is a CAS, so the pair linearizes — a
        // concurrent patch either lands before the delete (and is what gets
        // removed) or fails its guard afterwards.
        let tombstone = AclTombstone {
            id: stored.subject_id.clone(),
            name: stored.id.clone(),
            last_security_version: stored.security_version,
            deleted_at: now_millis(),
            v: AclTombstone::version(),
        };
        if let Err(err) = store.collection::<AclTombstone>().create(&tombstone).await {
            tracing::warn!(user = %name, error = %err, "failed to record the acl tombstone");
        }
        col.delete(name).await?;
        self.users.write().remove(name);
        Ok(())
    }

    async fn user_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.users.read().keys().cloned().collect();
        names.sort();
        names
    }

    async fn get_user(&self, name: &str) -> Option<JsonValue> {
        let user = self.user_of(name).await?;
        let row = &user.row;
        Some(json!({
            "name": row.id,
            "enabled": row.enabled,
            "allow": row.allow,
            "deny": row.deny,
            "patterns": row.patterns,
            "snapshot": row.policy().map(|p| p.snapshot).unwrap_or_default(),
            "passwords": row.passwords.len(),
            "must_change_password": row.must_change_password,
            "unrestricted": user.principal.is_unrestricted(),
        }))
    }
}

/// The one-call form of installing [`UserAcl`] on an engine.
///
/// ```no_run
/// use acts::Engine;
/// use acts_acl::AclUsers;
///
/// # async fn run() -> acts::Result<()> {
/// let engine = Engine::builder().with_user_acl().start().await?;
/// # Ok(())
/// # }
/// ```
pub trait AclUsers {
    /// Install the store-backed user registry.
    fn with_user_acl(self) -> Self;
}

impl AclUsers for acts::EngineBuilder {
    fn with_user_acl(self) -> Self {
        self.set_acl(Arc::new(UserAcl::new()))
    }
}

/// The builtin admin row: unrestricted, with the given plaintext password as
/// its single Argon2id credential (encoded exactly once here).
fn admin_row(password: &str) -> Result<AclUser> {
    admin_row_with(password, false)
}

/// The builtin admin row on a store with no `ACTS_ADMIN_PASSWORD`: the
/// documented default password and the initial-state flag.
fn admin_row_default() -> Result<AclUser> {
    admin_row_with(DEFAULT_ADMIN_PASSWORD, true)
}

fn admin_row_with(password: &str, must_change_password: bool) -> Result<AclUser> {
    let now = now_millis();
    Ok(AclUser {
        id: ADMIN_USER.to_string(),
        subject_id: random_id(16),
        enabled: true,
        security_version: 1,
        document_revision: 1,
        passwords: vec![phc_encode(password)?],
        max_sessions: None,
        must_change_password,
        allow: vec!["*".to_string()],
        deny: Vec::new(),
        patterns: vec!["*".to_string()],
        snapshot: "{}".to_string(),
        create_time: now,
        update_time: now,
        timestamp: now,
        v: AclUser::version(),
    })
}

/// Drop empty entries from a pattern list (an empty pattern is a typo, not a
/// grant).
fn normalize_list(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A random hex string (tokens, salts) from the system random source.
fn random_id(len: usize) -> String {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    nanoid::nanoid!(len, &HEX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts::{AccessControl, MemoryStore};

    /// A registry bound to a fresh in-memory store; the test build ignores
    /// the host's `ACTS_ADMIN_PASSWORD`, so the bootstrap is deterministic.
    async fn acl() -> (UserAcl, Arc<Store>) {
        let store = Arc::new(Store::new(Arc::new(MemoryStore::new())));
        let acl = UserAcl::new();
        acl.load(store.clone()).await.unwrap();
        (acl, store)
    }

    async fn set_user(acl: &UserAcl, spec: UserSpec) {
        acl.set_user(&spec).await.unwrap();
    }

    #[tokio::test]
    async fn load_bootstraps_the_builtin_admin() {
        let (acl, store) = acl().await;
        assert_eq!(acl.user_names().await, vec![ADMIN_USER.to_string()]);

        // a second load does not duplicate or reset anything
        let reloaded = UserAcl::new();
        reloaded.load(store).await.unwrap();
        assert_eq!(reloaded.user_names().await, vec![ADMIN_USER.to_string()]);

        // its password is whatever was set up: a wrong one is refused
        assert!(
            reloaded
                .login(ADMIN_USER, "not-the-password")
                .await
                .is_err()
        );
        let view = reloaded.get_user(ADMIN_USER).await.unwrap();
        assert_eq!(view["unrestricted"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn login_issues_tokens_that_authenticate() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["op-pass".to_string()],
                allow: Some(vec!["@read".to_string()]),
                ..Default::default()
            },
        )
        .await;

        let tokens = acl.login("op", "op-pass").await.unwrap();
        assert!(tokens.token.starts_with("at_"));
        assert!(tokens.refresh_token.starts_with("rt_"));
        assert_eq!(tokens.expires_in, ACCESS_TOKEN_TTL_SECS);
        assert_eq!(tokens.refresh_expires_in, REFRESH_TOKEN_TTL_SECS);

        let principal = acl.authenticate(Some(&tokens.token)).unwrap();
        assert!(principal.is_authenticated());
        assert_eq!(principal.subject(), "op");
        principal.check("model:ls").unwrap();
        assert!(principal.check("model:deploy").is_err());
    }

    #[tokio::test]
    async fn a_wrong_password_and_an_unknown_user_answer_the_same() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["op-pass".to_string()],
                ..Default::default()
            },
        )
        .await;

        let wrong = acl.login("op", "nope").await.unwrap_err();
        let unknown = acl.login("ghost", "nope").await.unwrap_err();
        assert_eq!(wrong.to_string(), unknown.to_string());
        assert!(matches!(wrong, AclError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn refresh_rotates_and_kills_the_old_pair() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();

        let next = acl.refresh(&tokens.refresh_token).await.unwrap();
        assert_ne!(tokens.token, next.token);
        // the old access token died with the rotation...
        assert!(
            !acl.authenticate(Some(&tokens.token))
                .unwrap()
                .is_authenticated()
        );
        // ...and presenting the consumed refresh again is a replay: it is
        // refused and revokes the whole family, successor included
        assert!(acl.refresh(&tokens.refresh_token).await.is_err());
        assert!(
            !acl.authenticate(Some(&next.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(acl.refresh(&next.refresh_token).await.is_err());
        // a fresh login starts a new, live family
        let again = acl.login("op", "p").await.unwrap();
        assert!(
            acl.authenticate(Some(&again.token))
                .unwrap()
                .is_authenticated()
        );
    }

    #[tokio::test]
    async fn logout_revokes_the_session_once() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();
        assert!(acl.logout(&tokens.token).await.unwrap());
        assert!(
            !acl.authenticate(Some(&tokens.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(!acl.logout(&tokens.token).await.unwrap());
    }

    #[tokio::test]
    async fn sessions_and_users_survive_a_reload() {
        let (acl, store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                allow: Some(vec!["model:ls".to_string()]),
                patterns: Some(vec!["orders:*".to_string()]),
                snapshot: Some(HashMap::from([(
                    "secrets".to_string(),
                    vec!["$subject".to_string()],
                )])),
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();

        // a second registry over the same store reads both back
        let reloaded = UserAcl::new();
        reloaded.load(store).await.unwrap();
        let principal = reloaded.authenticate(Some(&tokens.token)).unwrap();
        assert!(principal.is_authenticated());
        assert_eq!(principal.subject(), "op");
        principal.check("model:ls").unwrap();
        principal.check_rn("orders:eu").unwrap();
        assert!(principal.scope_policy().allows("secrets", "op"));
        assert!(!principal.scope_policy().allows("secrets", "other"));
        assert!(reloaded.login("op", "p").await.is_ok());
    }

    #[tokio::test]
    async fn setuser_edits_in_place() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["one".to_string(), "two".to_string()],
                allow: Some(vec!["model:ls".to_string()]),
                ..Default::default()
            },
        )
        .await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                rm_passwords: vec!["one".to_string()],
                allow: Some(vec!["model:get".to_string(), "proc:ls".to_string()]),
                ..Default::default()
            },
        )
        .await;

        assert!(acl.login("op", "one").await.is_err());
        let tokens = acl.login("op", "two").await.unwrap();
        let principal = acl.authenticate(Some(&tokens.token)).unwrap();
        principal.check("proc:ls").unwrap();
        assert!(principal.check("model:ls").is_err());

        // a policy change applies to the live session at once
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                allow: Some(vec!["model:*".to_string()]),
                ..Default::default()
            },
        )
        .await;
        acl.authenticate(Some(&tokens.token))
            .unwrap()
            .check("model:rm")
            .unwrap();
    }

    #[tokio::test]
    async fn a_disabled_user_cannot_login_and_loses_its_sessions() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();
        assert!(
            acl.authenticate(Some(&tokens.token))
                .unwrap()
                .is_authenticated()
        );

        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await;
        assert!(acl.login("op", "p").await.is_err());
        assert!(
            !acl.authenticate(Some(&tokens.token))
                .unwrap()
                .is_authenticated()
        );
    }

    #[tokio::test]
    async fn del_user_removes_the_user_and_its_sessions() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();

        acl.del_user("op").await.unwrap();
        assert!(
            !acl.authenticate(Some(&tokens.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(acl.login("op", "p").await.is_err());
        assert!(acl.del_user("op").await.is_err());
        assert_eq!(acl.get_user("op").await, None);
    }

    #[tokio::test]
    async fn the_builtin_admin_is_immortal() {
        let (acl, _store) = acl().await;
        assert!(acl.del_user(ADMIN_USER).await.is_err());
        assert!(
            acl.set_user(&UserSpec {
                name: ADMIN_USER.to_string(),
                enabled: Some(false),
                ..Default::default()
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn an_invalid_spec_is_refused_before_anything_is_stored() {
        let (acl, _store) = acl().await;
        assert!(
            acl.set_user(&UserSpec {
                name: String::new(),
                ..Default::default()
            })
            .await
            .is_err()
        );
        assert!(
            acl.set_user(&UserSpec {
                name: "a/b".to_string(),
                ..Default::default()
            })
            .await
            .is_err()
        );
        assert!(
            acl.set_user(&UserSpec {
                name: "x".to_string(),
                allow: Some(vec!["@reed".to_string()]),
                ..Default::default()
            })
            .await
            .is_err()
        );
        assert!(acl.user_names().await == vec![ADMIN_USER.to_string()]);
    }

    #[tokio::test]
    async fn an_unknown_or_absent_token_is_anonymous() {
        let (acl, _store) = acl().await;
        assert!(!acl.authenticate(None).unwrap().is_authenticated());
        assert!(
            !acl.authenticate(Some("nonsense"))
                .unwrap()
                .is_authenticated()
        );
        assert!(!acl.anonymous().is_authenticated());
    }

    #[tokio::test]
    async fn a_password_rotation_keeps_both_working() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["old".to_string()],
                ..Default::default()
            },
        )
        .await;
        // add the new one, then remove the old: no downtime in between
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["new".to_string()],
                ..Default::default()
            },
        )
        .await;
        assert!(acl.login("op", "old").await.is_ok());
        assert!(acl.login("op", "new").await.is_ok());
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                rm_passwords: vec!["old".to_string()],
                ..Default::default()
            },
        )
        .await;
        assert!(acl.login("op", "old").await.is_err());
        assert!(acl.login("op", "new").await.is_ok());
    }

    #[tokio::test]
    async fn a_fresh_store_starts_the_admin_in_its_initial_state() {
        let (registry, store) = acl().await;
        // the documented default password works, and the account is flagged
        assert!(registry.password_change_required(ADMIN_USER));
        let first = registry
            .login(ADMIN_USER, DEFAULT_ADMIN_PASSWORD)
            .await
            .unwrap();
        assert!(
            registry
                .authenticate(Some(&first.token))
                .unwrap()
                .is_authenticated()
        );

        // the change replaces the default credential and clears the flag
        set_user(
            &registry,
            UserSpec {
                name: ADMIN_USER.to_string(),
                add_passwords: vec!["new-pass".to_string()],
                ..Default::default()
            },
        )
        .await;
        assert!(!registry.password_change_required(ADMIN_USER));
        // the default password is gone, and the bump killed the old session
        assert!(
            registry
                .login(ADMIN_USER, DEFAULT_ADMIN_PASSWORD)
                .await
                .is_err()
        );
        assert!(
            !registry
                .authenticate(Some(&first.token))
                .unwrap()
                .is_authenticated()
        );
        // the next login uses the new password
        let second = registry.login(ADMIN_USER, "new-pass").await.unwrap();
        assert!(
            registry
                .authenticate(Some(&second.token))
                .unwrap()
                .is_authenticated()
        );
        // the clearing persists across a reload of the same store
        let reloaded = UserAcl::new();
        reloaded.load(store).await.unwrap();
        assert!(!reloaded.password_change_required(ADMIN_USER));
        assert!(reloaded.login(ADMIN_USER, "new-pass").await.is_ok());
    }

    #[tokio::test]
    async fn passwords_stored_as_argon2_phc() {
        let (acl, store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let row = store.collection::<AclUser>().find("op").await.unwrap();
        assert_eq!(row.passwords.len(), 1);
        assert!(row.passwords[0].starts_with("$argon2id$"));
    }

    #[tokio::test]
    async fn an_oversized_password_is_refused_before_the_kdf() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let oversized = "x".repeat(MAX_PASSWORD_BYTES + 1);
        let refused = acl.login("op", &oversized).await.unwrap_err();
        let wrong = acl.login("op", "nope").await.unwrap_err();
        assert_eq!(refused.to_string(), wrong.to_string());
    }

    #[tokio::test]
    async fn repeated_failures_block_the_name() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        for _ in 0..5 {
            assert!(acl.login("op", "nope").await.is_err());
        }
        assert!(acl.budget_blocked("op"));
        // the lockout refuses even the correct password — in the same words
        assert!(acl.login("op", "p").await.is_err());
        assert!(!acl.budget_blocked("other"));
    }

    #[tokio::test]
    async fn debug_never_leaks_tokens() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();
        let rendered = format!("{tokens:?}");
        assert!(!rendered.contains(&tokens.token));
        assert!(!rendered.contains(&tokens.refresh_token));
    }

    #[tokio::test]
    async fn a_deleted_and_recreated_name_never_binds_old_sessions() {
        let (acl, store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let old = acl.login("op", "p").await.unwrap();

        acl.del_user("op").await.unwrap();
        // the old access token is dead, and so is its refresh
        assert!(
            !acl.authenticate(Some(&old.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(acl.refresh(&old.refresh_token).await.is_err());

        // the name comes back — a NEW security subject, same password
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        assert!(
            !acl.authenticate(Some(&old.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(acl.refresh(&old.refresh_token).await.is_err());
        // the new generation logs in and authenticates as its own subject
        let new = acl.login("op", "p").await.unwrap();
        assert!(
            acl.authenticate(Some(&new.token))
                .unwrap()
                .is_authenticated()
        );

        // the tombstone recorded the first generation
        let tombstones = store
            .collection::<AclTombstone>()
            .query(&Query {
                offset: 0,
                limit: 10,
                filter: Some(Filter {
                    r#type: FilterType::And,
                    exprs: vec![FilterExpr::Expr(Expr::eq("name", "op"))],
                }),
                order_by: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(tombstones.rows.len(), 1);
    }

    #[tokio::test]
    async fn a_forced_logout_invalidates_issued_sessions() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let old = acl.login("op", "p").await.unwrap();

        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                force_logout: Some(true),
                ..Default::default()
            },
        )
        .await;

        // access, refresh and re-login-with-old-refresh are all dead; only a
        // fresh login works
        assert!(
            !acl.authenticate(Some(&old.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(acl.refresh(&old.refresh_token).await.is_err());
        let new = acl.login("op", "p").await.unwrap();
        assert!(
            acl.authenticate(Some(&new.token))
                .unwrap()
                .is_authenticated()
        );
    }

    #[tokio::test]
    async fn refresh_after_a_disable_is_refused() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await;
        assert!(acl.refresh(&tokens.refresh_token).await.is_err());
        assert!(acl.login("op", "p").await.is_err());
    }

    #[tokio::test]
    async fn resolve_keeps_missing_and_invalid_apart() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();

        assert!(matches!(
            acl.resolve(None).await,
            acts::AuthenticationResult::MissingCredentials
        ));
        assert!(matches!(
            acl.resolve(Some("nonsense")).await,
            acts::AuthenticationResult::InvalidCredentials
        ));
        assert!(matches!(
            acl.resolve(Some(tokens.token.as_str())).await,
            acts::AuthenticationResult::Authenticated(_)
        ));
        // an expired-fake token is invalid, never anonymous
        assert!(matches!(
            acl.resolve(Some("at_deadbeef")).await,
            acts::AuthenticationResult::InvalidCredentials
        ));
    }

    #[tokio::test]
    async fn a_replayed_refresh_revokes_the_whole_family() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let first = acl.login("op", "p").await.unwrap();
        let second = acl.refresh(&first.refresh_token).await.unwrap();

        // the consumed refresh comes back: the family dies, successor included
        assert!(acl.refresh(&first.refresh_token).await.is_err());
        assert!(
            !acl.authenticate(Some(&second.token))
                .unwrap()
                .is_authenticated()
        );
        // and a fresh login starts a new, live family
        let third = acl.login("op", "p").await.unwrap();
        assert!(
            acl.authenticate(Some(&third.token))
                .unwrap()
                .is_authenticated()
        );
    }

    #[tokio::test]
    async fn concurrent_refresh_yields_exactly_one_success() {
        let (acl, _store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();
        let refresh_token = tokens.refresh_token.clone();
        let acl = Arc::new(acl);
        let successes = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..32 {
            let acl = Arc::clone(&acl);
            let refresh_token = refresh_token.clone();
            let successes = Arc::clone(&successes);
            handles.push(tokio::spawn(async move {
                if acl.refresh(&refresh_token).await.is_ok() {
                    successes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(successes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn sessions_over_the_cap_evict_the_oldest() {
        let (acl, store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                max_sessions: Some(2),
                ..Default::default()
            },
        )
        .await;

        let first = acl.login("op", "p").await.unwrap();
        let second = acl.login("op", "p").await.unwrap();
        // the third login evicts the oldest (the first) in the same breath
        let third = acl.login("op", "p").await.unwrap();

        let active = store
            .collection::<AclSession>()
            .query(&Query {
                offset: 0,
                limit: 10,
                filter: Some(Filter {
                    r#type: FilterType::And,
                    exprs: vec![FilterExpr::Expr(Expr::eq("user", "op"))],
                }),
                order_by: Vec::new(),
            })
            .await
            .unwrap();
        let active = active
            .rows
            .into_iter()
            .filter(|row| matches!(row.state, SessionState::Active))
            .count();
        assert_eq!(active, 2);
        assert!(
            !acl.authenticate(Some(&first.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(
            acl.authenticate(Some(&second.token))
                .unwrap()
                .is_authenticated()
        );
        assert!(
            acl.authenticate(Some(&third.token))
                .unwrap()
                .is_authenticated()
        );
    }

    #[tokio::test]
    async fn the_gc_pass_removes_expired_sessions() {
        let (acl, store) = acl().await;
        set_user(
            &acl,
            UserSpec {
                name: "op".to_string(),
                add_passwords: vec!["p".to_string()],
                ..Default::default()
            },
        )
        .await;
        let tokens = acl.login("op", "p").await.unwrap();

        // forge an already-expired session row next to the live one
        let now = now_millis();
        let expired = AclSession {
            id: sha256_hex("at_expired"),
            session_id: random_id(16),
            family_id: random_id(16),
            parent_session_id: None,
            user: "op".to_string(),
            subject_id: "subj".to_string(),
            security_version: 1,
            refresh: sha256_hex("rt_expired"),
            access_expiry: now - 1_000,
            refresh_expiry: now - 1_000,
            state: SessionState::Active,
            revision: 1,
            create_time: now,
            update_time: now,
            timestamp: now,
            v: AclSession::version(),
        };
        store
            .collection::<AclSession>()
            .create(&expired)
            .await
            .unwrap();

        assert_eq!(UserAcl::gc_pass(&store).await, 1);
        assert!(
            store
                .collection::<AclSession>()
                .find_opt(&expired.id)
                .await
                .unwrap()
                .is_none()
        );
        // the live session is untouched
        assert!(
            acl.authenticate(Some(&tokens.token))
                .unwrap()
                .is_authenticated()
        );
    }
}
