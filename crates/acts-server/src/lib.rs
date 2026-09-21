//! Engine construction for the acts server binary — shared with the
//! integration tests so they exercise exactly what `acts-server` runs.

use acts::{
    Config, ConfigLog, Engine, EngineBuilder, KvStore, MissingParamAction, SnapshotOptions,
    SnapshotPolicy,
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tracing_appender::rolling::{RollingFileAppender, Rotation};

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
}

impl DbType {
    pub fn as_str(self) -> &'static str {
        match self {
            DbType::Sled => "sled",
            DbType::Sqlite => "sqlite",
            DbType::Postgres => "postgres",
        }
    }
}

impl std::fmt::Display for DbType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The optional `[db]` config section. Missing → [`DbConfig::default`]
/// (sled under the config directory, with the exclusive lease on). Non-sled
/// backends need both `type` and `database_url`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DbConfig {
    /// Store backend to open; `sled` when omitted.
    #[serde(rename = "type")]
    pub kind: DbType,
    /// Backend location: directory (sled), file (sqlite) or connection URL
    /// (postgres). The `ACTS_DATABASE_URL` env var overrides it.
    pub database_url: Option<String>,
    /// Hold the database's exclusive lease while this server runs (default
    /// `true`). The lease is a row in the database itself, taken with an
    /// atomic compare-and-swap, and every write the engine makes is committed
    /// only while this instance still holds it — so a second server on the
    /// same database is refused at startup instead of duplicating recovery
    /// and scheduling, and one that loses the lease mid-run (a takeover after
    /// a stall longer than the TTL) has its writes refused and its engine
    /// stopped. `false` accepts the older contract: one writer per database is
    /// then the operator's to guarantee.
    pub lease: bool,
    /// How long an unrenewed lease stays valid — a suffixed duration
    /// (`"30s"`, `"5m"`) or bare seconds; default 30. A crashed holder blocks
    /// a restart for at most this long.
    #[serde(default, deserialize_with = "de_ttl_secs")]
    pub lease_ttl_secs: Option<u64>,
    /// How often the lease is renewed, in the same units as
    /// [`DbConfig::lease_ttl_secs`]; default 10, and at most half the TTL so
    /// two renewals can fail before the lease expires.
    #[serde(default, deserialize_with = "de_ttl_secs")]
    pub lease_renew_secs: Option<u64>,
    /// Instance id written into the lease row — the name another instance's
    /// `LeaseHeld` startup error reports. Default `<host>:<pid>`.
    pub lease_owner: Option<String>,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            kind: DbType::default(),
            database_url: None,
            lease: true,
            lease_ttl_secs: None,
            lease_renew_secs: None,
            lease_owner: None,
        }
    }
}

/// The lease settings of one server, resolved and validated from `[db]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseConfig {
    /// Whether the server must hold the exclusive database lease.
    pub enabled: bool,
    /// How long an unrenewed lease stays valid.
    pub ttl: Duration,
    /// How often the holder renews it.
    pub renew: Duration,
    /// The instance id the lease row names.
    pub owner: String,
}

impl DbConfig {
    /// Resolve `[db]`'s lease settings, or report a combination that cannot
    /// work (a renewal that is not well inside the TTL would drop the lease
    /// under normal jitter, so it is rejected rather than tuned).
    pub fn lease_config(&self) -> acts::Result<LeaseConfig> {
        let ttl = self.lease_ttl_secs.unwrap_or(DEFAULT_LEASE_TTL_SECS);
        let renew = self.lease_renew_secs.unwrap_or(DEFAULT_LEASE_RENEW_SECS);
        if !self.lease {
            return Ok(LeaseConfig {
                enabled: false,
                ttl: Duration::from_secs(ttl),
                renew: Duration::from_secs(renew),
                owner: self.owner(),
            });
        }
        if ttl == 0 || renew == 0 {
            return Err(acts::ActError::Config(
                "[db].lease_ttl_secs and [db].lease_renew_secs must be greater than 0".to_string(),
            ));
        }
        if renew.saturating_mul(2) > ttl {
            return Err(acts::ActError::Config(format!(
                "[db].lease_renew_secs ({renew}s) must be at most half of \
                 [db].lease_ttl_secs ({ttl}s): a renewal that is not well inside the TTL \
                 drops the lease under ordinary scheduling jitter"
            )));
        }
        Ok(LeaseConfig {
            enabled: true,
            ttl: Duration::from_secs(ttl),
            renew: Duration::from_secs(renew),
            owner: self.owner(),
        })
    }

    fn owner(&self) -> String {
        self.lease_owner
            .clone()
            .filter(|owner| !owner.is_empty())
            .unwrap_or_else(default_lease_owner)
    }
}

/// `[db].lease_ttl_secs` / `[db].lease_renew_secs` default.
const DEFAULT_LEASE_TTL_SECS: u64 = 30;
/// `[db].lease_renew_secs` default — a third of the default TTL.
const DEFAULT_LEASE_RENEW_SECS: u64 = 10;

/// The default instance id in the lease row: the host and the process id, so
/// a `LeaseHeld` startup error names the machine and pid an operator has to
/// look at.
fn default_lease_owner() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string());
    format!("{host}:{}", std::process::id())
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
# unacknowledged message deliveries, resumes parked/queued processes, and
# re-drives scheduler outbox records no job owns any more (a `next`
# propagation whose job ended without closing it is retried on this tick).
tick_interval_secs = 15

# Max times an unacknowledged message delivery is re-sent before it turns
# into an Error that needs manual handling (msg:resend / msg:clear). 0
# disables the retry limit.
max_message_retry_times = 20

# Max times a single tree node can be executed inside one process. Protects
# against unbounded task creation from a step that loops back on itself or
# through a cyclic `next`. 0 disables the check.
max_node_run_times = 1000

# Number of scheduler task lanes. Independent pids can execute concurrently;
# tasks for the same pid are pinned to one lane and remain FIFO ordered.
scheduler_workers = 4

# Maximum scheduler jobs buffered in memory, split evenly across the lanes
# (at least one job each). A lane that is full spills task/next work to the
# durable outbox instead of buffering it, so this is the real in-memory
# backlog bound.
scheduler_queue_cap = 4096

# Number of concurrent store-writer shards. Independent pids are persisted
# concurrently, one queue consumer per shard; the writes of one pid always
# land in the same shard and keep their enqueue order.
store_writer_workers = 4

# Maximum store-writer backlog, split evenly across the shards (at least one
# op each). When a shard runs out of room the writer refuses new work with a
# QueueFull error (the producer falls back to its durable overflow path, or
# the request fails) until the backlog drains, so a backed-up store is never
# fed more work; the bookkeeping of in-flight work waits for room instead.
store_writer_queue_cap = 16384

# [log] — file logging: hourly rolling acts.log files under dir, at level
# (the ACTS_LOG env var overrides level at runtime).
#
# max_files bounds the disk the logs can grow to: the hourly files kept under
# dir. The oldest is deleted at every rotation, and stale files are pruned
# once at startup, so a restart also reclaims the space (at least 2 files are
# kept: the current one and the previous). Default 168 (one week of hourly
# files); 0 keeps every file.
[log]
dir = '@ACTS_DIR@/log'
level = "INFO"
max_files = 168

# storage backend. The default is sled, stored under the config directory.
# Other backends need both `type` and `database_url` in this [db] table
# (the ACTS_DATABASE_URL env var overrides database_url):
#   type = "sqlite"   database_url = "acts.db"
#   type = "postgres" database_url = "postgres://user:pass@host:5432/acts"
#
# Exclusive database lease (on unless it is turned off). The engine's document
# locks, write lanes, scheduler lanes and recovery claims are process-local, so
# two servers on one database would both recover the same rows and run the same
# work. With `lease = true` the server takes a lease row in the database itself
# — `__acts_lease__`, claimed with an atomic compare-and-swap and renewed every
# `lease_renew_secs` — and commits every write only while it still holds it:
#   * a second server on the same database fails its startup with a
#     `LeaseHeld` error naming the instance that holds it, instead of starting
#     an engine that duplicates recovery and scheduling;
#   * an instance whose lease is taken over (its renewals failed for longer
#     than `lease_ttl_secs`) has every write refused with `LeaseLost` and its
#     engine stopped, so it can never overwrite the new holder's rows;
#   * a holder's fence rises with every acquisition — across crashes and
#     restarts — and a write carrying a stale fence is refused inside the same
#     atomic write that would have committed it.
# A graceful stop releases the lease, so a rolling restart takes over at once;
# a crashed holder blocks a restart for at most `lease_ttl_secs`.
#
# `lease = false` accepts the older contract instead: one writer per database
# is yours to guarantee (separate databases per server, or coordination
# outside the engine).
#
# lease_ttl_secs / lease_renew_secs accept bare seconds or a suffixed duration
# ("30s", "5m"); `lease_renew_secs` must be at most half the TTL. `lease_owner`
# is the instance id another server's startup error names (default
# "<host>:<pid>").
[db]
type = "sled"
database_url = '@ACTS_DIR@/data'
lease = true
# lease_ttl_secs = 30
# lease_renew_secs = 10
# lease_owner = "acts-1"

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

# how many messages may wait for one `on_message` subscriber (default 128). The
# queue is the only backlog an RPC has: a message that does not fit is never
# awaited on, and a subscriber whose queue is full has its stream ended instead
# of growing a waiting task per message. Its unacked deliveries of processes
# that have not settled stay in the store, so the retry timer re-sends them
# when the client subscribes again.
# queue_size = 128

# web service — acts-plugin-web. Serves the HTTP API (/api/*) and workflow
# hooks (/hooks/{model-id}:{trigger-id}). Only started when this [web]
# section exists.
[web]
# port the web server listens on
port = 10082

# how many messages may wait for one SSE subscriber (default 100). The queue is
# the only backlog a stream has: a message that does not fit is never awaited
# on, and a subscriber whose queue is full is disconnected instead of growing a
# waiting task per message. Its unacked deliveries of processes that have not
# settled stay in the store, so the retry timer re-sends them when the client
# subscribes again.
# queue_size = 100

# access control — optional. The section's presence turns enforcement ON;
# without it every request is allowed (the pre-ACL behaviour).
#
# A request's token selects a role; the role's allow/deny action-name globs
# decide what it may do, and `deny` always wins. Because the section is the
# opt-in, a request with no token (or an unknown one) is refused unless
# `default_role` names a role. Tokens are matched by sha256 hex digest: store
# `sha256:<64 hex digits>` to keep the clear text out of this file, or write
# the token itself and let the server hash it.
#
# `snapshot` narrows which scopes of a target a subject owns; `$subject` is
# the role name, so `["$subject"]` means "my own scope only". A process
# started through an action carries its caller's rules, and the scheduler
# re-checks them at every seal — a workflow cannot read another subject's
# sealed data even when started with someone else's `uid`.
# `workdir` is a root: it gives each process its own directory (`<workdir>/<pid>`)
# and confines the process there, and the process id becomes a path segment, so a
# pid that is not one safe component is refused. It applies to every role unless
# the role sets its own. Acts read that directory through `Context::workdir()`,
# and a script reads the same one as `$env.WORK_DIR` (engine-owned: a write to
# that name is dropped); `acts.app.shell` mounts it as the root of the script's
# filesystem, so `/` inside the script is that directory (HOME, PWD, TMPDIR and
# ACTS_WORKDIR point at it) and the rest of the host is not part of the
# filesystem the script was given. The directory is removed with the process's
# rows, once the process finished and every delivery of its messages settled — so
# nothing left in it outlives the run (a run that must keep a file has to export
# it), and a finished run whose row is kept (an errored delivery awaiting a manual
# retry) keeps its directory as well. Omitted means no directory control, and a
# shell act then runs on an in-memory filesystem with no host behind it.
#
# Transport credentials:
#   gRPC  — `authorization: Bearer <token>` metadata
#   HTTP  — `authorization: Bearer <token>` header
#   NATS  — the `token` field of the action JSON body
#
# [acl]
# Without this section the engine is anonymous and catalogue-only: callers may
# list and get models and packages, and nothing else (no other read, no write,
# no control, no admin action, no snapshot scope, no subscription). Add the
# section to name your callers — the smallest useful one is the `token`
# shorthand below — or write `enabled = false` to lift the limits on purpose.
#
# role applied to an absent/unknown token; omit to refuse such requests
# default_role = "guest"
#
# shorthand: one token with unrestricted access (the `requirepass` equivalent)
# token = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
#
# [[acl.role]]
# name = "operator"
# tokens = ["sha256:2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"]
# allow = ["model:ls", "model:get", "proc:ls", "proc:get", "task:*", "msg:ls",
#          "msg:ack", "msg:sub", "snap:get", "snap:ls", "acl:whoami"]
# deny = ["model:rm", "pack:publish"]
# workdir = "/srv/acts"
#
# [[acl.role]]
# name = "guest"
# tokens = ["sha256:..."]
# allow = ["model:ls", "acl:whoami"]

# http package — acts-package-http rules for every `acts.core.http` act.
# Outbound requests are blocked by default when the target is a
# loopback/private/link-local address or a cloud metadata endpoint.
# [http]
# allow requests only to these hosts (empty = any host); "*.example.com"
# matches subdomains but not example.com itself
# allowed-hosts = ["api.example.com", "*.example.org"]
# opt in for internal/on-prem endpoints; metadata stays blocked
# allow-private-addresses = false
# maximum response body bytes; larger bodies fail the act
# max-response-bytes = 67108864
# connect timeout in ms; must be 1..=300000
# connect-timeout-ms = 10000
# whole-request timeout in ms, including reading the body; must be 1..=3600000.
# Neither timeout can be disabled, and an act's own `timeout-ms` may only
# tighten this value, never widen it.
# timeout-ms = 30000

# shell package — acts-package-shell. The script runs in bashkit's virtual bash
# (no PowerShell, Nushell or POSIX `sh`: `shell: bash` is the only accepted
# value) with the run's own directory as its filesystem root. Two glob lists
# over the whole script text; `deny` wins, and an empty pair means no
# restriction. A refused script fails the act before anything runs.
# [shell]
# allow = ["ls", "ls *", "cat *.txt"]
# deny = ["*rm -rf*", "*sudo *"]
# deadline of one shell act in ms; must be 1..=3600000. The interpreter is
# stopped and the act fails when it is reached, so a script that waits forever
# cannot hold a scheduler lane. Cannot be disabled.
# timeout-ms = 300000
# bytes captured from each of stdout and stderr before the act fails and the
# shell is killed; must be 1..=67108864. Cannot be disabled.
# max-output-bytes = 1048576

# nats service — acts-plugin-nats. Only connected when this [nats] section
# exists. Server actions are request/reply on "<subject>.cmd"; engine events
# are forwarded per the [[nats.channels]] entries below.
# [nats]
# broker url to connect to
# url = "nats://localhost:4222"
# action/event subject prefix
# subject = "acts"
# deadline of one acts.app.pubsub.nats act in ms; must be 1..=3600000. A
# subscribe that never receives a message, or a broker that never answers,
# fails the act instead of holding its scheduler lane. Cannot be disabled.
# timeout-ms = 30000
# how many actions received on "<subject>.cmd" may execute at once (default 256,
# 0 selects the default). Beyond it an action is refused to its caller instead
# of being started: a task per message made a busy subject unbounded work — the
# deploys, starts, store writes and outbound calls they run grew with whatever
# kept publishing.
# max_in_flight = 256

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

/// Prefix of the hourly rolling log files: the appender writes
/// `<dir>/acts.log.<YYYY-MM-DD-HH>` (UTC), which is also how older files from
/// a previous run are recognized for pruning.
const LOG_FILE_PREFIX: &str = "acts.log";

/// Open the hourly rolling log file appender described by `[log]`: creates
/// `dir` and applies the configured retention
/// ([`ConfigLog::retained_files`]), so a long-running server deletes its
/// oldest hourly file at every rotation — and once at startup — instead of
/// keeping every hour it ever logged.
///
/// The appender writes to the current hour's file as soon as it is built, so
/// the file exists on disk before the first event is logged.
pub fn log_file_appender(log: &ConfigLog) -> std::io::Result<RollingFileAppender> {
    std::fs::create_dir_all(&log.dir).map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("failed to create log dir {}: {err}", log.dir),
        )
    })?;
    let mut builder = RollingFileAppender::builder()
        .rotation(Rotation::HOURLY)
        .filename_prefix(LOG_FILE_PREFIX);
    if let Some(max_files) = log.retained_files() {
        builder = builder.max_log_files(max_files);
    }
    builder.build(&log.dir).map_err(std::io::Error::other)
}

/// A database this server may run on, with the exclusive lease that makes it
/// the only writer of that database.
///
/// [`OpenedStore::store`] is what [`engine_builder`] takes: with the lease on
/// it is a [`acts::FencedStore`] view, so the engine's every durable mutation
/// is committed only while this instance holds the lease.
#[derive(Clone)]
pub struct OpenedStore {
    /// The store to hand the engine.
    pub store: Arc<dyn KvStore>,
    lease: Option<Arc<acts::DbLease>>,
    renew: Duration,
}

impl OpenedStore {
    /// The lease this instance holds, or `None` when `[db].lease = false`.
    pub fn lease(&self) -> Option<&Arc<acts::DbLease>> {
        self.lease.as_ref()
    }

    /// Renew the lease while `engine` runs, and stop `engine` when it is lost.
    ///
    /// The keeper renews every `[db].lease_renew_secs`. If a renewal reports
    /// the lease as taken over (or its deadline passes while the store is
    /// unreachable), the lease is marked lost — every further write is refused
    /// with [`acts::ActError::LeaseLost`] — and the engine's shutdown token is
    /// cancelled, so the scheduler's lanes, the store writer and the timers
    /// stop admitting work. On a graceful shutdown the keeper releases the
    /// lease, so a rolling restart takes over immediately instead of waiting
    /// out the TTL.
    ///
    /// Returns `None` when the lease is disabled: nothing keeps an instance
    /// apart from another writer then.
    pub fn keep_lease(&self, engine: &acts::Engine) -> Option<acts::LeaseKeeper> {
        let lease = self.lease.clone()?;
        Some(acts::LeaseKeeper::start(
            lease,
            self.renew,
            engine.shutdown_token(),
        ))
    }
}

/// Open the KvStore backend selected by the `[db]` config, and take the
/// database's exclusive lease unless `[db].lease = false`.
///
/// `database_url` resolution: the `ACTS_DATABASE_URL` env var, then
/// `[db].database_url`, then the default `<config_dir>/data` for sled.
/// Any other type without a url is an error.
///
/// Fails — instead of starting a second engine over one database — when
/// another live instance holds the lease ([`acts::ActError::LeaseHeld`],
/// naming it).
pub async fn open_store(config_dir: &Path, db: &DbConfig) -> acts::Result<OpenedStore> {
    let url = database_url(config_dir, db)?;
    let raw: Arc<dyn KvStore> = match db.kind {
        DbType::Sled => {
            ensure_store_dir(&url)?;
            Arc::new(acts_store::SledStore::open(&url)?)
        }
        DbType::Sqlite => {
            ensure_store_dir(&url)?;
            Arc::new(acts_store::SqliteStore::open(&url).await?)
        }
        DbType::Postgres => Arc::new(acts_store::PostgresStore::open(&url).await?),
    };

    let lease_config = db.lease_config()?;
    if !lease_config.enabled {
        tracing::warn!(
            "[db].lease = false: nothing keeps a second server off this database. \
             One writer per database is yours to guarantee."
        );
        return Ok(OpenedStore {
            store: raw,
            lease: None,
            renew: lease_config.renew,
        });
    }
    let lease =
        match acts::DbLease::acquire(raw.clone(), &lease_config.owner, lease_config.ttl).await? {
            Some(lease) => lease,
            None => {
                return Err(acts::ActError::LeaseHeld(lease_holder_text(&raw).await));
            }
        };
    let store = lease.fenced();
    Ok(OpenedStore {
        store,
        lease: Some(Arc::new(lease)),
        renew: lease_config.renew,
    })
}

/// Describe the live lease that blocked a startup, for the operator reading
/// the error. Never fails: an unreadable row is still a reason not to start.
async fn lease_holder_text(store: &Arc<dyn KvStore>) -> String {
    let Ok(Some(bytes)) = store.one(acts::LEASE_KEY).await else {
        return "the database lease row is held".to_string();
    };
    match serde_json::from_slice::<acts::LeaseRecord>(&bytes) {
        Ok(record) => record.describe(),
        Err(_) => "the database lease row is unreadable".to_string(),
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
    /// seconds; unset never expires, `0` is a zero-length window (never
    /// valid). Expired entries are dropped on read and by a periodic sweep.
    /// A value above `acts::MAX_TTL_SECS` cannot be represented as a
    /// deadline and fails the engine startup.
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
    n.checked_mul(factor)
        .ok_or_else(|| format!("invalid ttl '{text}': the duration overflows the seconds range"))
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
///
/// A malformed `snapshot` section (for example a scalar where the
/// `[[snapshot]]` array of tables is expected) is reported as an error so
/// the server fails its startup instead of panicking.
pub fn engine_builder(
    config: &Config,
    store: Arc<dyn KvStore>,
    plugins: &ServerPlugins,
) -> acts::Result<EngineBuilder> {
    let mut builder = Engine::builder().set_config(config).set_store(store);
    if config.has("snapshot") {
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot")?;
        for target in &targets {
            let options: SnapshotOptions = target.clone().into();
            options.validate()?;
            builder = builder.add_snapshot(&target.name, options);
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
        .add_package::<acts_package_nats::NatsPackage>()
        .add_package::<acts_package_javascript::CodePackage>();
    if plugins.nats && config.has("nats") {
        builder = builder.add_plugin(&acts_plugin_nats::NatsPlugin::new());
    }
    Ok(builder)
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

        // engine_builder accepts a [[snapshot]] config and pre-registers the
        // targets through EngineBuilder::add_snapshot.
        engine_builder(
            &config,
            Arc::new(MemoryStore::new()),
            &ServerPlugins {
                grpc: false,
                web: false,
                nats: false,
            },
        )
        .unwrap();

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
    fn malformed_snapshot_config_fails_engine_build_without_panicking() {
        // `snapshot` present but not a [[snapshot]] array of tables: the
        // build must surface the config error instead of panicking, so a
        // deployment typo becomes a diagnosable startup failure.
        for body in [
            "snapshot = \"bad\"\n",
            "snapshot = 3\n",
            "[snapshot]\nname = \"p\"\n",
        ] {
            let (dir, path) = write_config(body);
            let config = Config::create(&path).unwrap();

            let err = engine_builder(
                &config,
                Arc::new(MemoryStore::new()),
                &ServerPlugins::default(),
            )
            .err()
            .unwrap_or_else(|| panic!("config {body:?} must fail engine construction"));
            let text = err.to_string();
            assert!(
                text.contains("snapshot"),
                "error for {body:?} should name the offending key: {text}"
            );
            assert!(matches!(err, acts::ActError::Config(_)), "{err:?}");

            std::fs::remove_file(&path).ok();
            std::fs::remove_dir(&dir).ok();
        }
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
        for bad in ["5x", "", "1.5h", "abc", "-2s", "18446744073709551615d"] {
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

    /// The lease is on unless a deployment turns it off, and its knobs are
    /// validated where they are read: a renewal that is not well inside the
    /// TTL would drop the lease under ordinary jitter, so it fails startup
    /// with a diagnostic instead of being silently tuned.
    #[test]
    fn lease_config_defaults_validates_and_turns_off() {
        let default = DbConfig::default().lease_config().unwrap();
        assert!(default.enabled, "the lease is on by default");
        assert_eq!(default.ttl, Duration::from_secs(30));
        assert_eq!(default.renew, Duration::from_secs(10));
        assert!(
            default.owner.ends_with(&format!(":{}", std::process::id())),
            "the default owner names the process: {}",
            default.owner
        );

        // Suffixed durations and bare seconds both parse, and the owner is
        // whatever the deployment named.
        let (_dir, path) = write_config(
            "[db]\ntype = \"sqlite\"\ndatabase_url = \"a.db\"\nlease_ttl_secs = \"5m\"\n\
             lease_renew_secs = 60\nlease_owner = \"acts-1\"\n",
        );
        let db = Config::create(&path)
            .unwrap()
            .get::<DbConfig>("db")
            .unwrap();
        let lease = db.lease_config().unwrap();
        assert_eq!(lease.ttl, Duration::from_secs(300));
        assert_eq!(lease.renew, Duration::from_secs(60));
        assert_eq!(lease.owner, "acts-1");

        // A renewal past half the TTL, or a zero, is refused.
        for body in [
            "[db]\nlease_ttl_secs = 30\nlease_renew_secs = 16\n",
            "[db]\nlease_ttl_secs = 30\nlease_renew_secs = 30\n",
            "[db]\nlease_ttl_secs = 1\nlease_renew_secs = 1\n",
            "[db]\nlease_ttl_secs = 0\nlease_renew_secs = 0\n",
        ] {
            let (dir, path) = write_config(body);
            let db = Config::create(&path)
                .unwrap()
                .get::<DbConfig>("db")
                .unwrap();
            let err = db.lease_config().unwrap_err();
            assert!(matches!(err, acts::ActError::Config(_)), "{body}: {err}");
            std::fs::remove_file(&path).ok();
            std::fs::remove_dir(&dir).ok();
        }
        // Half the TTL exactly is accepted.
        let (dir, path) = write_config("[db]\nlease_ttl_secs = 30\nlease_renew_secs = 15\n");
        let db = Config::create(&path)
            .unwrap()
            .get::<DbConfig>("db")
            .unwrap();
        assert_eq!(db.lease_config().unwrap().renew, Duration::from_secs(15));
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        // Turning the lease off is honored (and needs no valid renewal).
        let (dir, path) = write_config("[db]\nlease = false\nlease_ttl_secs = 0\n");
        let db = Config::create(&path)
            .unwrap()
            .get::<DbConfig>("db")
            .unwrap();
        assert!(!db.lease_config().unwrap().enabled);
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
            ..DbConfig::default()
        };
        let pg_no_url = DbConfig {
            kind: DbType::Postgres,
            database_url: None,
            ..DbConfig::default()
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
        let opened = open_store(&dir, &DbConfig::default()).await.unwrap();
        assert!(dir.join("data").is_dir(), "sled data dir under config dir");
        let store = opened.store.clone();
        store.put("k", b"v".to_vec()).await.unwrap();
        assert_eq!(store.one("k").await.unwrap(), Some(b"v".to_vec()));

        store.delete("k").await.unwrap();
        // The default config takes the database's lease before handing the
        // store to the engine: the mutations above went through its fence.
        let lease = opened
            .lease()
            .expect("the default config leases the database");
        assert_eq!(lease.fence(), 1);
        lease.release().await.unwrap();
        let err = store.put("k", b"v".to_vec()).await.unwrap_err();
        assert!(
            matches!(err, acts::ActError::LeaseLost),
            "a released lease stops writing: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two servers over one database: the second startup is refused while the
    /// first holds the lease, and accepted after it hands it back — with a
    /// higher fence, so its writes can never be confused with the first's.
    ///
    /// SQLite is the backend that exercises this in-process (sled holds its own
    /// file lock, so a second open of the same directory never gets as far as
    /// the lease check): each `open_store` builds its own pool, and the
    /// guarded batches below are committed by different connections in ONE
    /// order, which is what a multi-process deployment does.
    #[tokio::test]
    async fn a_second_server_is_refused_the_leased_database() {
        let (dir, _) = write_config("");
        let db = DbConfig {
            kind: DbType::Sqlite,
            database_url: Some(dir.join("acts.db").to_string_lossy().into_owned()),
            lease_owner: Some("first".to_string()),
            ..DbConfig::default()
        };

        let first = open_store(&dir, &db).await.unwrap();
        assert_eq!(first.lease().unwrap().fence(), 1);
        first.store.put("held", b"first".to_vec()).await.unwrap();

        let second = DbConfig {
            lease_owner: Some("second".to_string()),
            ..db.clone()
        };
        let err = match open_store(&dir, &second).await {
            Ok(_) => panic!("a second server must not start on a leased database"),
            Err(err) => err,
        };
        assert!(matches!(err, acts::ActError::LeaseHeld(_)), "got: {err}");
        assert!(
            err.to_string().contains("owner=first"),
            "the error names the holder: {err}"
        );

        // A graceful stop hands the lease back, and the next startup takes it
        // with a strictly higher fence.
        first.lease().unwrap().release().await.unwrap();
        let third = open_store(&dir, &second).await.unwrap();
        assert_eq!(third.lease().unwrap().fence(), 2);
        // The new holder writes; the old instance's fence is stale.
        third.store.put("held", b"third".to_vec()).await.unwrap();
        assert_eq!(
            first.store.one("held").await.unwrap(),
            Some(b"third".to_vec())
        );
        let err = first
            .store
            .put("held", b"first".to_vec())
            .await
            .unwrap_err();
        assert!(matches!(err, acts::ActError::LeaseLost), "got: {err}");
        assert_eq!(
            first.store.one("held").await.unwrap(),
            Some(b"third".to_vec()),
            "the stale instance's write did not land"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Scratch log dir, wiped so each case starts from a known state. The dir
    /// itself is created by [`log_file_appender`].
    fn log_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("acts-log-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    fn log_files(dir: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        let entry = entry.unwrap();
                        entry
                            .metadata()
                            .unwrap()
                            .is_file()
                            .then(|| entry.file_name().to_string_lossy().into_owned())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Write `count` stale hourly log files, as a previous run left them.
    fn stale_log_files(dir: &Path, count: usize) {
        std::fs::create_dir_all(dir).unwrap();
        for hour in 1..=count {
            std::fs::write(
                dir.join(format!("{LOG_FILE_PREFIX}.2020-01-01-{hour:02}")),
                b"old\n",
            )
            .unwrap();
        }
    }

    #[test]
    fn log_file_appender_prunes_stale_hourly_files() {
        // 10 hourly files from earlier runs, retention of 3: opening the
        // appender prunes down to the limit (the appender keeps max_files - 1
        // existing files and then opens the current hour's file), so a server
        // that restarts after hours of logging does not keep growing its log
        // directory.
        let dir = log_test_dir("retain");
        stale_log_files(&dir, 10);

        let appender = log_file_appender(&ConfigLog {
            dir: dir.to_string_lossy().into_owned(),
            level: "INFO".to_string(),
            max_files: Some(3),
        })
        .unwrap();

        let files = log_files(&dir);
        assert_eq!(files.len(), 3, "kept {files:?}");
        drop(appender);

        // max_files = 0 keeps every file
        let dir = log_test_dir("unlimited");
        stale_log_files(&dir, 10);
        let appender = log_file_appender(&ConfigLog {
            dir: dir.to_string_lossy().into_owned(),
            level: "INFO".to_string(),
            max_files: Some(0),
        })
        .unwrap();
        assert_eq!(
            log_files(&dir).len(),
            11,
            "10 stale files plus the current one"
        );
        drop(appender);

        std::fs::remove_dir_all(&dir).ok();
    }
}
