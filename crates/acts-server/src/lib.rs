//! Engine construction for the acts server binary — shared with the
//! integration tests so they exercise exactly what `acts-server` runs.

use acts::{Config, Engine, KvStore, MissingParamAction, SnapshotOptions, SnapshotPolicy};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Which plugins the built engine registers.
#[derive(Debug, Clone, Default)]
pub struct ServerPlugins {
    /// gRPC server plugin (default port 10080 unless `[grpc]` sets one).
    pub grpc: bool,
    /// web plugin.
    pub web: bool,
    /// NATS plugin — only registered when the config has a `[nats]` section.
    pub nats: bool,
}

impl ServerPlugins {
    /// Everything the shipped `acts-server` binary registers.
    pub fn full() -> Self {
        Self {
            grpc: true,
            web: true,
            nats: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    /// Local sled database (default) — `database_url` is a directory path.
    #[default]
    Sled,
    /// SQLite file — `database_url` is a file path.
    Sqlite,
    /// PostgreSQL — `database_url` is a connection URL.
    Postgres,
    /// Redis — `database_url` is a connection URL.
    Redis,
    /// NATS JetStream KV — `database_url` is a broker URL.
    Nats,
}

impl DbType {
    pub fn as_str(self) -> &'static str {
        match self {
            DbType::Sled => "sled",
            DbType::Sqlite => "sqlite",
            DbType::Postgres => "postgres",
            DbType::Redis => "redis",
            DbType::Nats => "nats",
        }
    }
}

impl std::fmt::Display for DbType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The optional `[db]` config section. Missing → [`DbConfig::default`]
/// (sled under the config directory). Non-sled backends need both `type`
/// and `database_url`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DbConfig {
    /// Store backend to open; `sled` when omitted.
    #[serde(rename = "type")]
    pub kind: DbType,
    /// Backend location: directory (sled), file (sqlite) or connection URL
    /// (postgres/redis/nats). The `ACTS_DATABASE_URL` env var overrides it.
    pub database_url: Option<String>,
}

/// Name of the config file inside the config directory.
pub const CONFIG_FILE_NAME: &str = "acts.toml";

/// Env var that overrides the default `~/.acts` config directory.
pub const ACTS_CONFIG_DIR_ENV: &str = "ACTS_CONFIG_DIR";

/// Env var that overrides the configured `[db].database_url`.
pub const ACTS_DATABASE_URL_ENV: &str = "ACTS_DATABASE_URL";

const DEFAULT_CONFIG_DIR_NAME: &str = ".acts";
/// Marker in the embedded default config replaced by the config dir path.
const CONFIG_DIR_MARKER: &str = "@ACTS_DIR@";

/// Default `acts.toml`, embedded in the binary so a `cargo install
/// acts-server` binary can auto-create the config on first start without any
/// shipped template file.
const DEFAULT_ACTS_TOML: &str = r#"# acts-server default configuration.
#
# This file is created automatically on the first start when it does not
# exist. `@ACTS_DIR@` is replaced with the absolute config directory
# (normally ~/.acts, or the directory set by the ACTS_CONFIG_DIR env var).
# Relative paths in an override file are resolved from the working directory.

# Max number of processes kept resident in memory at the same time. When the
# limit is reached a new process is persisted and parked, then started as
# soon as a resident process finishes and frees a slot.
cache_cap = 1024

# Engine scan interval in seconds: the periodic sweep that re-sends
# unacknowledged message deliveries and resumes parked/queued processes.
tick_interval_secs = 15

# Max times an unacknowledged message delivery is re-sent before it turns
# into an Error that needs manual handling (msg:resend / msg:clear). 0
# disables the retry limit.
max_message_retry_times = 20

# Max times a single tree node can be executed inside one process. Protects
# against unbounded task creation from a step that loops back on itself or
# through a cyclic `next`. 0 disables the check.
max_node_run_times = 1000

# [log] — file logging: hourly rolling acts.log files under dir, at level
# (the ACTS_LOG env var overrides level at runtime).
[log]
dir = '@ACTS_DIR@/log'
level = "INFO"

# storage backend. The default is sled, stored under the config directory.
# Other backends need both `type` and `database_url` in this [db] table
# (the ACTS_DATABASE_URL env var overrides database_url):
#   type = "sqlite"   database_url = "acts.db"
#   type = "postgres" database_url = "postgres://user:pass@host:5432/acts"
#   type = "redis"    database_url = "redis://127.0.0.1:6379"
#   type = "nats"     database_url = "nats://127.0.0.1:4222"
[db]
type = "sled"
database_url = '@ACTS_DIR@/data'

# snapshot-backed sealed-data targets.
# Data is fed through the web/grpc/nats snapshot APIs and sealed into tasks
# at their prepare under each target's policy:
#   policy     per_proc (default) = one value frozen per process lineage
#              per_task           = every task reads the latest value
#   scope      task param names whose values join with '/' into the scope
#              key (default: one global scope per target)
#   on_missing skip (default) | error
#   ttl        how long an entry stays valid after its last refresh —
#              suffixed duration ("2s", "5m", "3h", "1d") or bare seconds
#              (default: never expire)
[[snapshot]]
name = "profile"
policy = "per_proc"
scope = ["uid"]

[[snapshot]]
name = "secrets"
scope = ["uid"]
policy = "per_task"
ttl = "1h"

# grpc service — acts-plugin-grpc. Endpoint used by acts-cli and the
# acts-channel client libraries.
[grpc]
# port the gRPC server listens on
port = 10080

# web service — acts-plugin-web. Serves the HTTP API (/api/*) and workflow
# hooks (/hooks/{model-id}:{trigger-id}). Only started when this [web]
# section exists.
[web]
# port the web server listens on
port = 10082

# nats service — acts-plugin-nats. Only connected when this [nats] section
# exists. Server actions are request/reply on "<subject>.cmd"; engine events
# are forwarded per the [[nats.channels]] entries below.
# [nats]
# broker url to connect to
# url = "nats://localhost:4222"
# action/event subject prefix
# subject = "acts"

# [[nats.channels]]
# forward engine events matching these filters to the given subject:
#   id      channel name (for ack/redelivery bookkeeping)
#   subject subject the events are published to
#   type    event type filter  ("*" = all)
#   state   task state filter  ("*" = all)
#   uses    action/uses filter ("*" = all)
# id = "c1"
# subject = "acts.evt"
# type = "*"
# state = "*"
# uses = "*"
"#;

/// The acts-server config directory: `$ACTS_CONFIG_DIR`, else `~/.acts`
/// (`$HOME`, or `$USERPROFILE` on Windows). Falls back to a `.acts`
/// directory next to the working directory when no home is resolvable.
pub fn config_dir() -> PathBuf {
    match std::env::var_os(ACTS_CONFIG_DIR_ENV) {
        Some(dir) => PathBuf::from(dir),
        None => home_dir()
            .map(|home| home.join(DEFAULT_CONFIG_DIR_NAME))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_DIR_NAME)),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Path of the config file under `dir`.
pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// Create `dir` and, when `dir/acts.toml` is absent, write the embedded
/// default config there (its `data`/`log` paths point into the config
/// directory, made absolute at write time). Returns the config file path.
pub fn ensure_default_config(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = config_path(dir);
    if !path.exists() {
        let dir_text = dir.to_string_lossy().replace('\\', "/");
        std::fs::write(
            &path,
            DEFAULT_ACTS_TOML.replace(CONFIG_DIR_MARKER, &dir_text),
        )?;
    }
    Ok(path)
}

/// Open the KvStore backend selected by the `[db]` config.
///
/// `database_url` resolution: the `ACTS_DATABASE_URL` env var, then
/// `[db].database_url`, then the default `<config_dir>/data` for sled.
/// Any other type without a url is an error.
pub async fn open_store(config_dir: &Path, db: &DbConfig) -> acts::Result<Arc<dyn KvStore>> {
    let url = database_url(config_dir, db)?;
    match db.kind {
        DbType::Sled => {
            ensure_store_dir(&url)?;
            Ok(Arc::new(acts_store::SledStore::open(&url)?))
        }
        DbType::Sqlite => {
            ensure_store_dir(&url)?;
            Ok(Arc::new(acts_store::SqliteStore::open(&url).await?))
        }
        DbType::Postgres => Ok(Arc::new(acts_store::PostgresStore::open(&url).await?)),
        DbType::Redis => Ok(Arc::new(acts_store::RedisStore::open(&url).await?)),
        DbType::Nats => Ok(Arc::new(acts_store::NatsStore::open(&url).await?)),
    }
}

fn database_url(config_dir: &Path, db: &DbConfig) -> acts::Result<String> {
    if let Some(url) = std::env::var(ACTS_DATABASE_URL_ENV)
        .ok()
        .filter(|url| !url.is_empty())
    {
        return Ok(url);
    }
    if let Some(url) = db.database_url.as_deref().filter(|url| !url.is_empty()) {
        return Ok(url.to_string());
    }
    if db.kind == DbType::Sled {
        return Ok(config_dir.join("data").to_string_lossy().into_owned());
    }
    Err(acts::ActError::Config(format!(
        "db type '{}' requires database_url in [db] or the {ACTS_DATABASE_URL_ENV} env var",
        db.kind
    )))
}

/// Create the parent directory of a file-backed store path. Connection
/// URLs (`scheme://…`) and `:memory:` are left untouched.
fn ensure_store_dir(url: &str) -> acts::Result<()> {
    if url.contains("://") || url == ":memory:" {
        return Ok(());
    }
    if let Some(parent) = Path::new(url).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| {
            acts::ActError::Store(format!(
                "failed to create store directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    Ok(())
}

/// One `[[snapshot]]` target entry from the engine config file.
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotConfig {
    /// target name — the sealed-data name the scheduler injects into task
    /// env as `$<name>` (e.g. `profile`, `secrets`).
    pub name: String,
    /// when the cache value is frozen into task sealed data
    /// (`per_proc` default: once per task lineage; `per_task`: every task
    /// re-reads the latest value at its prepare).
    #[serde(default)]
    pub policy: SnapshotPolicy,
    /// task param names whose values join with `/` into this target's scope
    /// key. Empty: one global scope per target.
    #[serde(default)]
    pub scope: Vec<String>,
    /// what happens when a key param or the snapshot data is absent at a
    /// task's prepare (`skip` default: seal nothing; `error`: fail the task).
    #[serde(default)]
    pub on_missing: MissingParamAction,
    /// how long an entry stays valid after its last refresh — a suffixed
    /// duration like `"2s"`, `"5m"`, `"3h"`, `"1d"`, or a bare number of
    /// seconds; unset never expires. Expired entries are dropped on read
    /// and by a periodic sweep.
    #[serde(default, deserialize_with = "de_ttl_secs")]
    pub ttl: Option<u64>,
}

impl From<SnapshotConfig> for SnapshotOptions {
    fn from(c: SnapshotConfig) -> Self {
        Self {
            policy: c.policy,
            scope: c.scope,
            on_missing: c.on_missing,
            ttl_secs: c.ttl,
        }
    }
}

/// Parse a suffixed TTL like `2s`, `5m`, `3h`, `1d` into seconds.
fn parse_ttl_secs(text: &str) -> Result<u64, String> {
    let (factor, digits) = match text.as_bytes().last() {
        Some(b's') => (1, &text[..text.len() - 1]),
        Some(b'm') => (60, &text[..text.len() - 1]),
        Some(b'h') => (3600, &text[..text.len() - 1]),
        Some(b'd') => (86_400, &text[..text.len() - 1]),
        _ => {
            return Err(format!(
                "invalid ttl '{text}': expected a suffixed duration like \
                 \"2s\", \"5m\", \"3h\" or \"1d\", or a bare number of seconds"
            ));
        }
    };
    let n: u64 = digits.parse().map_err(|_| {
        format!("invalid ttl '{text}': the value before the unit must be a whole number")
    })?;
    Ok(n * factor)
}

/// Deserialize a snapshot `ttl` config value: suffixed duration string
/// (`2s`/`5m`/`3h`/`1d`) or a bare integer counting seconds.
fn de_ttl_secs<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct TtlVisitor;

    impl serde::de::Visitor<'_> for TtlVisitor {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(
                "a suffixed duration (\"2s\", \"5m\", \"3h\", \"1d\") or a \
                 bare number of seconds",
            )
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            u64::try_from(v)
                .map(Some)
                .map_err(|_| E::custom("ttl must be a non-negative number"))
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            parse_ttl_secs(v).map(Some).map_err(E::custom)
        }
    }

    deserializer.deserialize_any(TtlVisitor)
}

/// Build the acts-server engine: core packages plus the transport plugins
/// selected by `plugins`. The NATS plugin is additionally gated on the
/// config having a `[nats]` section, so a server without one never tries to
/// reach a broker. `[[snapshot]]` targets declared in the config are
/// pre-registered with [`acts::EngineBuilder::add_snapshot`] so the
/// scheduler seals their values under the configured policy/scope.
pub fn build_engine(config: &Config, store: Arc<dyn KvStore>, plugins: &ServerPlugins) -> Engine {
    let mut builder = Engine::builder().set_config(config).set_store(store);
    if config.has("snapshot") {
        let targets = config
            .get::<Vec<SnapshotConfig>>("snapshot")
            .expect("config 'snapshot' must be a [[snapshot]] array of target tables");
        for target in &targets {
            builder = builder.add_snapshot(&target.name, target.clone().into());
        }
    }
    if plugins.grpc {
        builder = builder.add_plugin(&acts_plugin_grpc::GrpcPlugin::new());
    }
    if plugins.web && config.has("web") {
        builder = builder.add_plugin(&acts_plugin_web::WebPlugin::new());
    }
    builder = builder
        .add_package::<acts_package_http::HttpPackage>()
        .add_package::<acts_package_shell::ShellPackage>()
        .add_package::<acts_package_state::StatePackage>()
        .add_package::<acts_package_nats::NatsPackage>();
    if plugins.nats && config.has("nats") {
        builder = builder.add_plugin(&acts_plugin_nats::NatsPlugin::new());
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts::MemoryStore;

    fn write_config(body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let thread = std::thread::current()
            .name()
            .unwrap_or("t")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let dir =
            std::env::temp_dir().join(format!("acts-snap-cfg-{}-{thread}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("acts.toml");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn snapshot_config_parses_and_maps_to_options() {
        let (dir, path) = write_config(
            r#"
[[snapshot]]
name = "profile"
policy = "per_proc"

[[snapshot]]
name = "secrets"
policy = "per_task"
scope = ["unit"]
on_missing = "error"
ttl = "5m"
"#,
        );
        let config = Config::create(&path).unwrap();

        // build_engine accepts a [[snapshot]] config and pre-registers the
        // targets through EngineBuilder::add_snapshot (a parse or mapping
        // error would panic here).
        build_engine(
            &config,
            Arc::new(MemoryStore::new()),
            &ServerPlugins {
                grpc: false,
                web: false,
                nats: false,
            },
        );

        // parsed entries translate 1:1 into snapshot registration options
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot").unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "profile");
        assert_eq!(targets[0].policy, SnapshotPolicy::PerProc);
        assert_eq!(targets[1].name, "secrets");
        assert_eq!(targets[1].policy, SnapshotPolicy::PerTask);
        assert_eq!(targets[1].scope, vec!["unit".to_string()]);
        assert_eq!(targets[1].on_missing, MissingParamAction::Error);
        assert_eq!(targets[1].ttl, Some(300));

        let options: SnapshotOptions = targets[1].clone().into();
        assert_eq!(options.policy, SnapshotPolicy::PerTask);
        assert_eq!(options.scope, vec!["unit".to_string()]);
        assert_eq!(options.on_missing, MissingParamAction::Error);
        assert_eq!(options.ttl_secs, Some(300));

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn snapshot_config_defaults_fill_missing_fields() {
        let (dir, path) = write_config("[[snapshot]]\nname = \"profile\"\n");
        let config = Config::create(&path).unwrap();
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot").unwrap();
        assert_eq!(targets[0].name, "profile");
        assert_eq!(targets[0].policy, SnapshotPolicy::PerProc);
        assert_eq!(targets[0].scope, Vec::<String>::new());
        assert_eq!(targets[0].on_missing, MissingParamAction::Skip);
        assert_eq!(targets[0].ttl, None);

        let options: SnapshotOptions = targets[0].clone().into();
        assert_eq!(options.policy, SnapshotPolicy::PerProc);
        assert_eq!(options.scope, Vec::<String>::new());
        assert_eq!(options.on_missing, MissingParamAction::Skip);
        assert_eq!(options.ttl_secs, None);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn snapshot_config_ttl_duration_units() {
        // suffixed durations parse to seconds
        for (input, want) in [
            ("2s", 2),
            ("5m", 300),
            ("3h", 10_800),
            ("1d", 86_400),
            ("10d", 864_000),
        ] {
            assert_eq!(parse_ttl_secs(input).unwrap(), want, "ttl {input}");
        }
        // a bare integer in the config counts seconds
        let (dir, path) = write_config("[[snapshot]]\nname = \"cron\"\nttl = 3600\n");
        let config = Config::create(&path).unwrap();
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot").unwrap();
        assert_eq!(targets[0].ttl, Some(3600));
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        // malformed values fail loudly instead of silently disabling ttl
        for bad in ["5x", "", "1.5h", "abc", "-2s"] {
            assert!(parse_ttl_secs(bad).is_err(), "ttl {bad} should be rejected");
        }
    }

    #[test]
    fn ensure_default_config_creates_the_file_once() {
        let thread = std::thread::current()
            .name()
            .unwrap_or("t")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let dir = std::env::temp_dir().join(format!(
            "acts-default-cfg-{}-{}",
            std::process::id(),
            thread
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = ensure_default_config(&dir).unwrap();
        assert_eq!(path, dir.join(CONFIG_FILE_NAME));

        // first call writes the template with the config dir baked in
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(CONFIG_DIR_MARKER));
        assert!(text.contains(&dir.to_string_lossy().replace('\\', "/")));
        // the file parses as a full server config with the sled default
        let config = Config::create(&path).unwrap();
        assert!(config.has("db"));
        assert!(config.has("web"));
        assert_eq!(config.get::<DbConfig>("db").unwrap().kind, DbType::Sled);

        // a second call must not rewrite an existing (possibly edited) file
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_default_config(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            stamp,
            "existing config file must not be overwritten"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn db_config_defaults_and_parses_other_types() {
        assert_eq!(DbConfig::default().kind, DbType::Sled);
        assert_eq!(DbConfig::default().database_url, None);

        let (dir, path) =
            write_config("[db]\ntype = \"postgres\"\ndatabase_url = \"postgres://h/db\"\n");
        let config = Config::create(&path).unwrap();
        let db = config.get::<DbConfig>("db").unwrap();
        assert_eq!(db.kind, DbType::Postgres);
        assert_eq!(db.database_url.as_deref(), Some("postgres://h/db"));
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn database_url_prefers_config_then_sled_default() {
        let config_dir = Path::new("/tmp/acts");
        let sled = DbConfig::default();
        let pg = DbConfig {
            kind: DbType::Postgres,
            database_url: Some("postgres://h/db".to_string()),
        };
        let pg_no_url = DbConfig {
            kind: DbType::Postgres,
            database_url: None,
        };

        // no env set (guarded): config url wins, sled falls back under the
        // config dir, any other type without a url is an error
        if std::env::var_os(ACTS_DATABASE_URL_ENV).is_none() {
            assert_eq!(
                database_url(config_dir, &sled).unwrap(),
                config_dir.join("data").to_string_lossy()
            );
            assert_eq!(
                database_url(config_dir, &pg).unwrap(),
                "postgres://h/db".to_string()
            );
            assert!(database_url(config_dir, &pg_no_url).is_err());
        }
    }

    #[tokio::test]
    async fn open_store_default_sled_creates_dir_and_roundtrips() {
        let (dir, _) = write_config("");
        let store = open_store(&dir, &DbConfig::default()).await.unwrap();
        assert!(dir.join("data").is_dir(), "sled data dir under config dir");
        store.put("k", b"v".to_vec()).await.unwrap();
        assert_eq!(store.get("k").await.unwrap(), Some(b"v".to_vec()));

        store.delete("k").await.unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
