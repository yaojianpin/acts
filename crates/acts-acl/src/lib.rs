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
//! unrestricted, with the password in [`ADMIN_PASSWORD_ENV`] or a generated
//! one that is printed to the log once. It cannot be deleted or disabled, so
//! an engine can never lock itself out of its own registry.
//!
//! ## Sessions
//!
//! `acl:login` answers a short-lived access token and a long-lived refresh
//! token. Only their sha256 digests are stored; a refresh rotates the pair and
//! kills the old one (a refresh token is single-use). Sessions are rows, so a
//! server restart does not log every caller out.

#![doc = include_str!("../README.md")]

use acts::query::Query;
use acts::{
    AccessControl, AclError, ActError, DbCollectionIden, LoginTokens, Principal, Result, Store,
    UserPolicy, UserSpec,
};
use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

/// The builtin administrator every engine starts with: unrestricted, with a
/// generated password (or [`ADMIN_PASSWORD_ENV`]). It cannot be deleted or
/// disabled, so an engine can never lock itself out of its own ACL.
pub const ADMIN_USER: &str = "admin";

/// Environment variable that sets the builtin admin password at first start
/// (when the `acl_users` collection is still empty).
pub const ADMIN_PASSWORD_ENV: &str = "ACTS_ADMIN_PASSWORD";

/// How long an access token answers, in seconds.
pub const ACCESS_TOKEN_TTL_SECS: i64 = 3600;
/// How long a refresh token may rotate a session, in seconds.
pub const REFRESH_TOKEN_TTL_SECS: i64 = 7 * 24 * 3600;

const USERS_PREFIX: &str = "acl_users";
const SESSIONS_PREFIX: &str = "acl_sessions";

fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

/// One access-control user, stored in the `acl_users` collection.
#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct AclUser {
    /// username; doubles as the subject a request is attributed to
    pub id: String,
    /// an off user authenticates nothing, `acl:login` included
    pub enabled: bool,
    /// salted password hashes (`salt$sha256(salt:password)`)
    pub passwords: Vec<String>,
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
        0
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(ActError::Store(format!(
            "unsupported acl user version: {v}"
        )))
    }
}

/// One login session, stored in the `acl_sessions` collection.
///
/// The tokens themselves are never stored — only their sha256 digests. The
/// access digest is the row id; the refresh digest is held inside the row so
/// a refresh can be rotated one-time.
#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct AclSession {
    /// sha256(access token)
    pub id: String,
    /// username the token pair authenticates as
    pub user: String,
    /// sha256(refresh token)
    pub refresh: String,
    /// access-token expiry, millis since epoch
    pub access_expiry: i64,
    /// refresh-token expiry, millis since epoch
    pub refresh_expiry: i64,

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
        &["timestamp", "create_time", "update_time"]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
        &["timestamp", "create_time", "update_time"]
    }

    fn version() -> i32 {
        0
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(ActError::Store(format!(
            "unsupported acl session version: {v}"
        )))
    }
}

/// One user with its grants already compiled into a principal.
#[derive(Debug, Clone)]
struct CompiledUser {
    row: AclUser,
    principal: Principal,
}

/// One live session, keyed by the sha256 of its access token.
#[derive(Debug, Clone)]
struct Session {
    user: String,
    refresh_hash: String,
    access_expiry: i64,
    refresh_expiry: i64,
}

/// The store-backed user registry and session store.
///
/// Constructed before the engine knows its store (`UserAcl::new`), bound to
/// it when the engine starts.
pub struct UserAcl {
    store: RwLock<Option<Arc<Store>>>,
    users: RwLock<HashMap<String, Arc<CompiledUser>>>,
    /// access-token hash -> session
    sessions: RwLock<HashMap<String, Session>>,
    /// refresh-token hash -> access-token hash
    refresh: RwLock<HashMap<String, String>>,
}

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
    pub fn new() -> Self {
        Self {
            store: RwLock::new(None),
            users: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            refresh: RwLock::new(HashMap::new()),
        }
    }

    /// The users currently loaded, by name.
    fn store(&self) -> Option<Arc<Store>> {
        self.store.read().clone()
    }

    /// Mint a fresh token pair for `user` and persist the session. The store
    /// row lands first, so a session that cannot be persisted is never
    /// answered.
    async fn mint(&self, user: &str) -> std::result::Result<LoginTokens, AclError> {
        let now = now_millis();
        let token = format!("at_{}", random_id(32));
        let refresh_token = format!("rt_{}", random_id(32));
        let access_hash = sha256_hex(&token);
        let refresh_hash = sha256_hex(&refresh_token);
        let access_expiry = now + ACCESS_TOKEN_TTL_SECS * 1000;
        let refresh_expiry = now + REFRESH_TOKEN_TTL_SECS * 1000;

        if let Some(store) = self.store() {
            let row = AclSession {
                id: access_hash.clone(),
                user: user.to_string(),
                refresh: refresh_hash.clone(),
                access_expiry,
                refresh_expiry,
                create_time: now,
                update_time: now,
                timestamp: now,
                v: 0,
            };
            if let Err(err) = store.collection::<AclSession>().create(&row).await {
                return Err(AclError::Denied(format!(
                    "failed to persist the session: {err}"
                )));
            }
        }
        self.sessions.write().insert(
            access_hash.clone(),
            Session {
                user: user.to_string(),
                refresh_hash: refresh_hash.clone(),
                access_expiry,
                refresh_expiry,
            },
        );
        self.refresh.write().insert(refresh_hash, access_hash);

        Ok(LoginTokens {
            token,
            refresh_token,
            expires_in: ACCESS_TOKEN_TTL_SECS,
            refresh_expires_in: REFRESH_TOKEN_TTL_SECS,
        })
    }

    /// Drop a session (by access hash) from memory and the store.
    async fn revoke(&self, access_hash: &str) -> Result<bool> {
        let refresh_hash = {
            let mut sessions = self.sessions.write();
            sessions
                .remove(access_hash)
                .map(|session| session.refresh_hash)
        };
        let Some(refresh_hash) = refresh_hash else {
            return Ok(false);
        };
        self.refresh.write().remove(&refresh_hash);
        if let Some(store) = self.store() {
            store.collection::<AclSession>().delete(access_hash).await?;
        }
        Ok(true)
    }

    /// The principal a live session resolves to, if the session is unexpired
    /// and its user is still enabled.
    fn session_principal(&self, token: &str) -> Option<Principal> {
        let hash = sha256_hex(token);
        let (user_name, access_expiry) = {
            let sessions = self.sessions.read();
            let session = sessions.get(&hash)?;
            (session.user.clone(), session.access_expiry)
        };
        if access_expiry < now_millis() {
            return None;
        }
        let users = self.users.read();
        let user = users.get(&user_name)?;
        if !user.row.enabled {
            return None;
        }
        Some(user.principal.clone())
    }

    /// The user row behind a name, cloned out of the registry.
    fn row_of(&self, name: &str) -> Option<AclUser> {
        self.users.read().get(name).map(|user| user.row.clone())
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
            users.insert(name, Arc::new(CompiledUser { row, principal }));
        }

        let now = now_millis();
        let mut sessions = HashMap::new();
        let mut refresh = HashMap::new();
        for row in store
            .collection::<AclSession>()
            .query_all(&Query::new())
            .await?
        {
            // a session whose refresh token expired is dead; drop the row
            if row.refresh_expiry < now {
                let _ = store.collection::<AclSession>().delete(&row.id).await;
                continue;
            }
            refresh.insert(row.refresh.clone(), row.id.clone());
            sessions.insert(
                row.id.clone(),
                Session {
                    user: row.user.clone(),
                    refresh_hash: row.refresh.clone(),
                    access_expiry: row.access_expiry,
                    refresh_expiry: row.refresh_expiry,
                },
            );
        }

        // First start (or a store without users): the builtin admin. Its
        // password comes from the environment or is generated and printed
        // once — either way it is changeable with `acl:setuser`.
        if users.is_empty() {
            let password = match std::env::var(ADMIN_PASSWORD_ENV) {
                Ok(password) if !password.trim().is_empty() => password,
                _ => {
                    let password = random_id(16);
                    tracing::warn!(
                        user = ADMIN_USER,
                        password = %password,
                        "no acl user found: created the builtin '{ADMIN_USER}' user with a generated password (set {ADMIN_PASSWORD_ENV} to choose your own; change it anytime with acl:setuser)"
                    );
                    password
                }
            };
            let row = admin_row(&password);
            store.collection::<AclUser>().create(&row).await?;
            let principal = Principal::from_policy(&row.policy()?)?;
            users.insert(row.id.clone(), Arc::new(CompiledUser { row, principal }));
        }

        *self.users.write() = users;
        *self.sessions.write() = sessions;
        *self.refresh.write() = refresh;
        Ok(())
    }

    fn enabled(&self) -> bool {
        true
    }

    fn authenticate(&self, token: Option<&str>) -> std::result::Result<Principal, AclError> {
        if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty())
            && let Some(principal) = self.session_principal(token)
        {
            return Ok(principal);
        }
        Ok(Principal::anonymous())
    }

    async fn login(
        &self,
        user: &str,
        password: &str,
    ) -> std::result::Result<LoginTokens, AclError> {
        let compiled = {
            let users = self.users.read();
            users.get(user.trim()).cloned()
        };
        let Some(compiled) = compiled else {
            return Err(AclError::Unauthenticated(
                "invalid user or password".to_string(),
            ));
        };
        // one answer for "no such user", "off" and "wrong password": the
        // error must not tell an attacker which half it got
        if !compiled.row.enabled || !verify_password(&compiled.row.passwords, password) {
            return Err(AclError::Unauthenticated(
                "invalid user or password".to_string(),
            ));
        }
        self.mint(&compiled.row.id).await
    }

    async fn refresh(&self, refresh_token: &str) -> std::result::Result<LoginTokens, AclError> {
        let refresh_token = refresh_token.trim();
        if refresh_token.is_empty() {
            return Err(AclError::Unauthenticated(
                "a refresh token is required".to_string(),
            ));
        }
        let refresh_hash = sha256_hex(refresh_token);
        let access_hash = {
            let refresh = self.refresh.read();
            refresh.get(&refresh_hash).cloned()
        };
        let Some(access_hash) = access_hash else {
            return Err(AclError::Unauthenticated(
                "the refresh token is unknown or was already used".to_string(),
            ));
        };
        let (user, refresh_expiry) = {
            let sessions = self.sessions.read();
            let Some(session) = sessions.get(&access_hash) else {
                return Err(AclError::Unauthenticated(
                    "the session this refresh token belonged to is gone".to_string(),
                ));
            };
            (session.user.clone(), session.refresh_expiry)
        };
        if refresh_expiry < now_millis() {
            let _ = self.revoke(&access_hash).await;
            return Err(AclError::Unauthenticated(
                "the refresh token expired; login again".to_string(),
            ));
        }
        // single use: the old pair dies with the rotation, so a leaked refresh
        // token cannot be replayed
        let _ = self.revoke(&access_hash).await;
        self.mint(&user).await
    }

    async fn logout(&self, token: &str) -> Result<bool> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(false);
        }
        let hash = sha256_hex(token);
        let access_hash = {
            let refresh = self.refresh.read();
            refresh.get(&hash).cloned().unwrap_or(hash)
        };
        self.revoke(&access_hash).await
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

        // compute the next row against the current one, without holding the
        // lock across the store write
        let mut row = self.row_of(&name).unwrap_or_else(|| AclUser {
            id: name.clone(),
            enabled: true,
            create_time: now_millis(),
            ..Default::default()
        });
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
        for password in spec
            .rm_passwords
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
        {
            // every salted form of that plaintext goes
            row.passwords.retain(|entry| {
                entry
                    .split_once('$')
                    .map(|(salt, hash)| sha256_hex(&format!("{salt}:{password}")) != hash)
                    .unwrap_or(true)
            });
        }
        for password in spec.add_passwords.iter().map(|p| p.trim()) {
            if password.is_empty() {
                return Err(ActError::Config(
                    "acl user password cannot be empty".to_string(),
                ));
            }
            let salt = random_id(8);
            let hashed = format!("{salt}${}", sha256_hex(&format!("{salt}:{password}")));
            if !row.passwords.contains(&hashed) {
                row.passwords.push(hashed);
            }
        }

        // compile first: a bad pattern must fail before anything is stored
        let policy = row.policy()?;
        let principal = Principal::from_policy(&policy)?;
        let store = self
            .store()
            .ok_or_else(|| ActError::Store("the acl is not bound to a store".to_string()))?;
        let existed = self.users.read().contains_key(&name);
        row.update_time = now_millis();
        row.timestamp = row.update_time;
        if existed {
            store.collection::<AclUser>().update(&row).await?;
        } else {
            store.collection::<AclUser>().create(&row).await?;
        }
        self.users
            .write()
            .insert(name, Arc::new(CompiledUser { row, principal }));
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

        let removed = self.users.write().remove(name);
        if removed.is_none() {
            return Err(ActError::Config(format!(
                "acl user '{name}' does not exist"
            )));
        }
        store.collection::<AclUser>().delete(name).await?;

        // the deleted user's sessions die with it
        let dead: Vec<String> = {
            let sessions = self.sessions.read();
            sessions
                .iter()
                .filter(|(_, session)| session.user == name)
                .map(|(hash, _)| hash.clone())
                .collect()
        };
        for hash in dead {
            self.revoke(&hash).await?;
        }
        Ok(())
    }

    async fn user_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.users.read().keys().cloned().collect();
        names.sort();
        names
    }

    async fn get_user(&self, name: &str) -> Option<JsonValue> {
        let users = self.users.read();
        let user = users.get(name)?;
        let row = &user.row;
        Some(json!({
            "name": row.id,
            "enabled": row.enabled,
            "allow": row.allow,
            "deny": row.deny,
            "patterns": row.patterns,
            "snapshot": row.policy().map(|p| p.snapshot).unwrap_or_default(),
            "passwords": row.passwords.len(),
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

fn admin_row(password: &str) -> AclUser {
    let now = now_millis();
    AclUser {
        id: ADMIN_USER.to_string(),
        enabled: true,
        passwords: vec![salted(password)],
        allow: vec!["*".to_string()],
        deny: Vec::new(),
        patterns: vec!["*".to_string()],
        snapshot: "{}".to_string(),
        create_time: now,
        update_time: now,
        timestamp: now,
        v: 0,
    }
}

/// `password` with a fresh salt, in the stored `salt$hash` form.
fn salted(password: &str) -> String {
    let salt = random_id(8);
    format!("{salt}${}", sha256_hex(&format!("{salt}:{password}")))
}

/// Whether `password` matches any of the stored `salt$hash` entries.
fn verify_password(passwords: &[String], password: &str) -> bool {
    passwords
        .iter()
        .filter_map(|entry| entry.split_once('$'))
        .any(|(salt, hash)| sha256_hex(&format!("{salt}:{password}")) == hash)
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

    /// A registry bound to a fresh in-memory store.
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
        // ...and so did the old refresh token (single use)
        assert!(acl.refresh(&tokens.refresh_token).await.is_err());
        assert!(
            acl.authenticate(Some(&next.token))
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
}
