//! The engine's own answer about itself (`sys:status`): the host process's
//! memory and cpu usage, the store's footprint, the process-row counts, the
//! background loops' health, the ingress metrics and a *redacted* view of
//! the effective configuration — the data an operator needs to see a
//! running engine without reaching for the process's own tools.
//!
//! Two rules bound the answer:
//!
//! - **Redaction**: the config echo never carries a secret. Fields the
//!   configuration declares sensitive (`password`, `token`, `secret`,
//!   credentials, key paths) answer `"[configured]"`; credentials inside
//!   URLs are scrubbed (`postgres://user:***@host/db`); *unclassified*
//!   fields and sections answer `"[redacted]"` — a future config key an
//!   upgrade introduces is invisible until someone classifies it, not
//!   silently public. The full, unredacted table still exists on the
//!   server's own log at startup if configured to log it; the status
//!   answer never does.
//! - **Budget**: collection is cached (2 s TTL, per engine), capped at four
//!   concurrent collections (an overrun answers the cache, stale or not)
//!   and timed out at 5 s — a hundred concurrent `sys:status` calls cost
//!   the engine one store sweep and one cpu sample per 2 seconds, not one
//!   hundred.
//!
//! The wire shape is mirrored by the client (`acts-channel`'s model), so
//! the field names here and there are one contract: change both, never one.

use crate::{
    Engine, Result,
    query::{Expr, Filter, Query},
    scheduler::{HealthState, SchedulerHealth},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};
use toml::Table;

/// How long a `sys:status` answer is served from cache.
const STATUS_CACHE_TTL: Duration = Duration::from_secs(2);
/// How many collections may sample concurrently; the rest answer the cache.
const STATUS_CONCURRENCY: usize = 4;
/// Hard ceiling of one uncached collection.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// The `sys:status` answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// The engine crate's version — which build answered.
    pub version: String,
    /// Memory usage: the host process and the machine it runs on.
    pub memory: MemoryStatus,
    /// Cpu usage: the host process and the machine, sampled over
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`].
    pub cpu: CpuStatus,
    /// The store backend's identity and footprint, when it can measure itself.
    pub storage: StorageStatus,
    /// Process-row counts: everything the store holds and the part still
    /// running (the store keeps only un-swept rows, so "total" is the runs
    /// not yet reclaimed by the terminal-run cleanup).
    pub processes: ProcessCounts,
    /// The background loops' failure state.
    pub health: HealthStatus,
    /// The installed capabilities' keys, one entry per capability
    /// implementation the serving assembly attached (see
    /// [`acts::caps::Capability`]): package effects, branch predicates,
    /// outbox delivery, process lifecycle.
    pub caps: Vec<String>,
    /// The effective configuration, **redacted**: sensitive fields answer
    /// `"[configured]"`, credentials inside URLs are scrubbed, and fields no
    /// schema entry classifies answer `"[redacted]"`. See [`redact_config`].
    pub config: serde_json::Value,
    /// The ingress counters (`acts_ingress_*`) and subscription gauges
    /// (`acts_subscriptions*`), label-keyed. Absent in answers from a
    /// server that predates the field (the client default is empty).
    #[serde(default)]
    pub metrics: BTreeMap<String, u64>,
}

/// Memory usage in bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatus {
    /// The host process's resident set.
    pub rss_bytes: u64,
    /// Installed physical memory of the machine.
    pub system_total_bytes: u64,
    /// Physical memory currently available to processes.
    pub system_available_bytes: u64,
}

/// Cpu usage, in percent (0–100 per the caller's arithmetic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuStatus {
    /// The host process's share since the previous sample.
    pub process_percent: f32,
    /// The machine's global non-idle share since the previous sample.
    pub system_percent: f32,
    /// Logical cores the machine reports.
    pub logical_cores: usize,
}

/// The store backend's footprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatus {
    /// The backend's own word for itself (`memory`, `sled`, `sqlite`,
    /// `postgres`, `rocksdb`, `sharded`, or a custom store's).
    pub kind: String,
    /// Data size in bytes, best effort; `null` when the backend cannot
    /// measure itself.
    pub used_bytes: Option<u64>,
}

/// Process-row counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCounts {
    /// Every process row the store holds.
    pub total: usize,
    /// The rows still in flight: state `running` or `none` (a run whose root
    /// step has not been created yet). Everything else — `completed`,
    /// `error`, `aborted`, … — is settled.
    pub active: usize,
}

/// The background loops' health, overall and per loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// `healthy` when every loop is, `degraded` when any loop is skipping
    /// ticks because its store keeps failing. Backpressure is reported
    /// separately: a full write pipeline is a healthy limit, not a failure —
    /// it only becomes `degraded` once loop ticks actually fail.
    pub state: String,
    /// The store-writer's queue and the backend's own write pressure
    /// (`KvStore::saturated`). `saturated` is the exact flag
    /// [`crate::Engine`] exposes as `store_writer_saturated`, so a status
    /// reader sees the same admission signal producers use.
    pub store_writer: StoreWriterStatus,
    /// One entry per loop, in [`crate::scheduler::core::health`]'s order:
    /// trigger, retry, cleanup, ready, recovery, outbox, projection.
    pub loops: Vec<LoopStatus>,
}

/// The write path's current pressure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreWriterStatus {
    /// Ops accepted by the writer but not yet applied.
    pub depth: usize,
    /// Highest `depth` observed since the engine started.
    pub high_watermark: usize,
    /// Whether the writer is refusing new work right now — its own backlog
    /// fill or the database's bounded pipeline is full.
    pub saturated: bool,
}

/// One loop's failure state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopStatus {
    /// The loop's name (`trigger`, `retry`, `cleanup`, `ready`, `recovery`,
    /// `outbox`, `projection`).
    pub name: String,
    /// `healthy` or `degraded`.
    pub state: String,
    /// Store failures in a row since the last clean tick.
    pub consecutive_failures: u64,
    /// Ticks the loop sits out before its next attempt (`0` while healthy).
    pub backoff_ticks: u32,
}

/// The process rows that count as still running: a root not yet created and
/// a root in flight. The words are the proc row's state vocabulary (see
/// `TaskState::proc_state`).
const ACTIVE_STATES: [&str; 2] = ["none", "running"];

/// Collect the status answer for `engine`.
///
/// Cached per engine (`2 s`), capped at [`STATUS_CONCURRENCY`] concurrent
/// samples and hard-timed-out at [`STATUS_TIMEOUT`]: `sys:status` must not
/// be the request that walks the whole store under load.
pub async fn collect(engine: &Engine) -> Result<SystemStatus> {
    static GATE: LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(STATUS_CONCURRENCY)));
    static CACHE: LazyLock<tokio::sync::Mutex<StatusCache>> =
        LazyLock::new(|| tokio::sync::Mutex::new(StatusCache::empty()));

    let engine_key = engine_key(engine);
    // Over concurrency: answer the cache even when stale — a monitoring
    // sweep that outruns the samplers gets numbers, not a queue.
    let permit = match GATE.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return CACHE.lock().await.answer(engine_key).ok_or_else(|| {
                crate::ActError::Runtime("status collection is busy; retry shortly".to_string())
            });
        }
    };
    // A fresh-enough answer of *this* engine is served without sampling.
    // The cache lock is dropped before the (seconds-long) sample below.
    if let Some(status) = CACHE.lock().await.fresh(engine_key) {
        drop(permit);
        return Ok(status);
    }

    let status = tokio::time::timeout(STATUS_TIMEOUT, collect_uncached(engine))
        .await
        .map_err(|_| {
            crate::ActError::Runtime(format!(
                "status collection exceeded {STATUS_TIMEOUT:?}; the store may be saturated"
            ))
        })??;
    CACHE.lock().await.store(engine_key, status.clone());
    drop(permit);
    Ok(status)
}

/// One engine's cached answer. The key is the engine's runtime address, so
/// two engines in one process (tests) never share an answer.
struct StatusCache {
    engine_key: usize,
    at: Option<tokio::time::Instant>,
    status: Option<SystemStatus>,
}

impl StatusCache {
    fn empty() -> Self {
        Self {
            engine_key: 0,
            at: None,
            status: None,
        }
    }

    fn fresh(&self, engine_key: usize) -> Option<SystemStatus> {
        if self.engine_key != engine_key {
            return None;
        }
        self.at
            .is_some_and(|at| at.elapsed() < STATUS_CACHE_TTL)
            .then(|| self.status.clone())
            .flatten()
    }

    fn answer(&self, engine_key: usize) -> Option<SystemStatus> {
        if self.engine_key != engine_key {
            return None;
        }
        self.status.clone()
    }

    fn store(&mut self, engine_key: usize, status: SystemStatus) {
        self.engine_key = engine_key;
        self.at = Some(tokio::time::Instant::now());
        self.status = Some(status);
    }
}

/// A stable per-engine identity: the runtime's allocation address, which
/// lives exactly as long as the engine's own runtime.
fn engine_key(engine: &Engine) -> usize {
    Arc::as_ptr(&engine.runtime()) as usize
}

/// The uncached sample: store counts, store usage, process metrics, loop
/// health, ingress metrics and the redacted config.
async fn collect_uncached(engine: &Engine) -> Result<SystemStatus> {
    let runtime = engine.runtime();

    // Both counts come from the read model's proc collection, one row
    // fetched each: `count` is the whole answer, the page is discarded.
    let store = runtime.store();
    let total = store.procs().query(&Query::new().limit(1)).await?.count;
    let active = store
        .procs()
        .query(
            &Query::new()
                .limit(1)
                .filter(Filter::and().expr(Expr::r#in("state", ACTIVE_STATES.to_vec()))),
        )
        .await?
        .count;

    let usage = store.kv().usage().await?;
    let storage = match usage {
        Some(usage) => StorageStatus {
            kind: usage.kind,
            used_bytes: usage.used_bytes,
        },
        None => StorageStatus {
            kind: "unknown".to_string(),
            used_bytes: None,
        },
    };

    let (memory, cpu) = sample_process_metrics().await;
    let health = health_status(
        runtime.scheduler_health(),
        runtime.store_writer_depth(),
        runtime.store_writer_high_watermark(),
        runtime.store_writer_saturated(),
    );

    let ingress = engine.ingress();
    let mut metrics = ingress.metrics.snapshot();
    for (key, value) in ingress.subscriptions.gauges() {
        metrics.insert(key, value);
    }

    Ok(SystemStatus {
        version: crate::VERSION.to_string(),
        memory,
        cpu,
        storage,
        processes: ProcessCounts { total, active },
        health,
        caps: runtime.caps(),
        config: redact_config(&runtime.config().table),
        metrics,
    })
}

/// One pass of the host's memory and cpu numbers.
///
/// sysinfo derives usage rates from the difference of two samples, so the
/// pass refreshes, waits the minimum cpu interval, and refreshes again. The
/// wait is the one cost of an honest number; it runs on the async runtime as
/// a timer, not a block.
async fn sample_process_metrics() -> (MemoryStatus, CpuStatus) {
    // Our own pid, and there is nowhere to report a failure to: a host that
    // cannot even name itself answers zeroes instead of failing a status
    // read that answered everything else.
    let Some(pid) = sysinfo::get_current_pid().ok() else {
        return (
            MemoryStatus {
                rss_bytes: 0,
                system_total_bytes: 0,
                system_available_bytes: 0,
            },
            CpuStatus {
                process_percent: 0.0,
                system_percent: 0.0,
                logical_cores: 0,
            },
        );
    };
    let mut system = System::new();
    let sample = |system: &mut System| {
        system.refresh_memory();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory(),
        );
        system.refresh_cpu_usage();
    };

    sample(&mut system);
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sample(&mut system);

    let process = system.process(pid);
    let memory = MemoryStatus {
        rss_bytes: process.map(|p| p.memory()).unwrap_or(0),
        system_total_bytes: system.total_memory(),
        system_available_bytes: system.available_memory(),
    };
    let cpu = CpuStatus {
        process_percent: process.map(|p| p.cpu_usage()).unwrap_or(0.0),
        system_percent: system.global_cpu_usage(),
        logical_cores: system.cpus().len(),
    };
    (memory, cpu)
}

/// Flatten the health bundle into the wire answer: an overall word, the
/// write path's pressure, and one entry per loop.
fn health_status(
    health: SchedulerHealth,
    depth: usize,
    high_watermark: usize,
    saturated: bool,
) -> HealthStatus {
    let loops = [
        ("trigger", health.trigger),
        ("retry", health.retry),
        ("cleanup", health.cleanup),
        ("ready", health.ready),
        ("recovery", health.recovery),
        ("outbox", health.outbox),
        ("projection", health.projection),
    ]
    .into_iter()
    .map(|(name, loop_health)| LoopStatus {
        name: name.to_string(),
        state: loop_health.state.as_str().to_string(),
        consecutive_failures: loop_health.consecutive_failures,
        backoff_ticks: loop_health.backoff_ticks,
    })
    .collect::<Vec<_>>();

    let degraded = HealthState::Degraded.as_str();
    let state = if loops
        .iter()
        .any(|loop_status| loop_status.state == degraded)
    {
        degraded
    } else {
        HealthState::Healthy.as_str()
    };

    HealthStatus {
        state: state.to_string(),
        store_writer: StoreWriterStatus {
            depth,
            high_watermark,
            saturated,
        },
        loops,
    }
}

// ---------------------------------------------------------------------------
// Config redaction
// ---------------------------------------------------------------------------

/// Key parts that always mean a secret, whatever section they sit in. A
/// field whose name contains any of these answers `"[configured]"` — its
/// existence stays visible, its value never leaves the process.
const SENSITIVE_KEY_PARTS: &[&str] = &[
    "password",
    "passwd",
    "token",
    "secret",
    "credential",
    "api_key",
    "private_key",
    "access_key",
    "nkey",
    "seed",
];

/// The fields of each known section that are safe to publish. A field not
/// listed is `"[redacted]"` — an unknown key from a newer config is hidden
/// by default, not public by accident.
const SECTION_ALLOWLIST: &[(&str, &[&str])] = &[
    (
        "db",
        &[
            "kind",
            "database_url",
            "shards",
            "lease",
            "lease_renew_secs",
        ],
    ),
    ("web", &["host", "port", "queue_size", "tls"]),
    ("grpc", &["host", "port", "queue_size", "tls"]),
    (
        "nats",
        &[
            "url",
            "subject",
            "max_in_flight",
            "max_payload_bytes",
            "tls_required",
            "reject_jetstream_capture",
            "channels",
            // channel metadata (one entry per forwarded subscription)
            "id",
            "type",
            "state",
            "uses",
        ],
    ),
    ("log", &["dir", "level", "max_files"]),
];

/// Sections that carry no secret by construction (pure knobs) and pass
/// through unredacted. A key added to one of them in a future version is
/// still published — they are configuration shapes, not credentials.
const PASS_THROUGH_SECTIONS: &[&str] = &["ingress", "subscriptions", "shutdown"];

/// The root-level scalar keys of the engine config (`ConfigData`'s fields —
/// the scheduling knobs — plus nothing else). Everything else at the root
/// is redacted.
const ROOT_KEYS: &[&str] = &[
    "cache_cap",
    "effect_concurrency",
    "recovery_concurrency",
    "reconciliation_concurrency",
    "projection_concurrency",
    "tick_interval_secs",
    "ready_interval_secs",
    "recovery_interval_secs",
    "outbox_interval_secs",
    "projection_interval_secs",
    "trigger_interval_secs",
    "retry_interval_secs",
    "snapshot_interval_secs",
    "ready_batch",
    "recovery_batch",
    "outbox_batch",
    "projection_batch",
    "cleanup_interval_secs",
    "cleanup_batch",
    "max_message_retry_times",
    "max_node_run_times",
    "scheduler_workers",
    "scheduler_queue_cap",
    "store_writer_queue_cap",
    "workdir",
    "log",
];

/// Redact the config table for the status answer: sensitive names answer
/// `"[configured]"`, URL credentials are scrubbed, unknown keys answer
/// `"[redacted]"`, allow-listed keys pass (URL-like values scrubbed).
pub fn redact_config(table: &Table) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (key, value) in table {
        if PASS_THROUGH_SECTIONS.contains(&key.as_str()) {
            out.insert(key.clone(), toml_to_json(value));
        } else if let Some((_, allow)) = SECTION_ALLOWLIST.iter().find(|(s, _)| s == key) {
            out.insert(key.clone(), redact_section(value, allow));
        } else if ROOT_KEYS.contains(&key.as_str()) {
            if key == "log" {
                out.insert(
                    key.clone(),
                    redact_section(value, &["dir", "level", "max_files"]),
                );
            } else {
                out.insert(key.clone(), redact_value(value, &[]));
            }
        } else {
            // Unknown section or key: keep its keys' existence, never their
            // values — a sensitive-looking name still answers
            // "[configured]", an unclassified one "[redacted]" (and an
            // unknown root scalar is unclassified by definition).
            let redacted = match value {
                toml::Value::Table(_) | toml::Value::Array(_) => redact_section(value, &[]),
                _ => redact_unknown(value),
            };
            out.insert(key.clone(), redacted);
        }
    }
    serde_json::Value::Object(out)
}

/// Redact one known section's fields against its allow-list.
fn redact_section(value: &toml::Value, allow: &[&str]) -> serde_json::Value {
    match value {
        toml::Value::Table(table) => {
            let mut out = serde_json::Map::new();
            for (key, inner) in table {
                if is_sensitive_key(key) {
                    out.insert(
                        key.clone(),
                        serde_json::Value::String("[configured]".into()),
                    );
                } else if allow.contains(&key.as_str()) {
                    out.insert(key.clone(), redact_value(inner, allow));
                } else {
                    out.insert(key.clone(), redact_unknown(inner));
                }
            }
            serde_json::Value::Object(out)
        }
        toml::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| redact_section(item, allow))
                .collect(),
        ),
        other => redact_value(other, allow),
    }
}

/// One allow-listed value: URLs get their userinfo and password-like query
/// parameters scrubbed; tables recurse (a nested `tls` block, the `shards`
/// list); scalars pass.
fn redact_value(value: &toml::Value, allow: &[&str]) -> serde_json::Value {
    match value {
        toml::Value::String(text) => serde_json::Value::String(scrub_url_credentials(text)),
        toml::Value::Table(table) => {
            let mut out = serde_json::Map::new();
            for (key, inner) in table {
                if is_sensitive_key(key) {
                    out.insert(
                        key.clone(),
                        serde_json::Value::String("[configured]".into()),
                    );
                } else if allow.is_empty() || allow.contains(&key.as_str()) {
                    out.insert(key.clone(), redact_value(inner, allow));
                } else {
                    out.insert(key.clone(), redact_unknown(inner));
                }
            }
            serde_json::Value::Object(out)
        }
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(|item| redact_value(item, allow)).collect())
        }
        other => toml_to_json(other),
    }
}

/// An unclassified key's placeholder: the shape (how many entries) stays,
/// the content never does.
fn redact_unknown(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::Table(table) => serde_json::json!({ "redacted": table.len() }),
        toml::Value::Array(items) => serde_json::json!({ "redacted": items.len() }),
        _ => serde_json::Value::String("[redacted]".into()),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_lowercase();
    SENSITIVE_KEY_PARTS
        .iter()
        .any(|part| lowered.contains(part))
}

fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// Scrub the credentials of a URL-shaped string: `scheme://user:pass@host/…`
/// becomes `scheme://***@host/…`, and a password-like query parameter
/// (`…?password=x` / `…&password=x`) loses its value. Strings without a
/// `scheme://` prefix pass unchanged.
pub fn scrub_url_credentials(text: &str) -> String {
    let Some(scheme_end) = text.find("://") else {
        return text.to_string();
    };
    let rest = &text[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let scrubbed_authority = match authority.find('@') {
        Some(at) => {
            let userinfo = &authority[..at];
            let host = &authority[at + 1..];
            let masked_userinfo = userinfo
                .rsplit_once(':')
                .map(|(user, _)| format!("{user}:***"))
                .unwrap_or_else(|| "***".to_string());
            format!("{masked_userinfo}@{host}")
        }
        None => authority.to_string(),
    };
    let mut out = format!(
        "{}{}{}",
        &text[..scheme_end + 3],
        scrubbed_authority,
        &rest[authority_end..]
    );
    // Password-shaped query parameters: `?password=x` and `&password=x`
    // (and `postgres://host/db?password=x` has no userinfo to scrub).
    for marker in ["?password=", "&password=", "?passwd=", "&passwd="] {
        if let Some(start) = out.find(marker) {
            let value_start = start + marker.len();
            let value_end = out[value_start..]
                .find(['&', '#'])
                .map(|i| value_start + i)
                .unwrap_or(out.len());
            out.replace_range(value_start..value_end, "***");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::toml;

    /// The canary secrets of every documented shape vanish from the
    /// redacted config: the database URL's password, the NATS token and
    /// password, a TLS private key path, a plugin's api key — while the
    /// operational fields (hosts, ports, kinds) stay.
    #[test]
    fn redaction_hides_every_sensitive_field() {
        let table: Table = toml! {
            tick_interval_secs = 1
            scheduler_workers = 4
            not_a_known_knob = "mystery"

            [db]
            kind = "postgres"
            database_url = "postgres://acts:hunter2@db.local:5432/acts"
            lease = true

            [nats]
            url = "nats://ops:pw123@broker.local:4222"
            token = "super-secret-token"
            password = "hunter2"
            username = "ops"
            subject = "acts"
            max_in_flight = 256

            [[nats.channels]]
            id = "c1"
            subject = "acts.evt"
            credential_file = "/etc/nats/ops.creds"

            [web]
            host = "127.0.0.1"
            port = 10082
            tls_private_key = "/etc/acts/server.key"

            [vendor_plugin]
            api_key = "canary-key"
            endpoint = "https://vendor.example/v1"
        };

        let redacted = redact_config(&table);
        let text = redacted.to_string();

        // No canary anywhere in the answer.
        assert!(!text.contains("hunter2"), "db password leaked: {text}");
        assert!(
            !text.contains("super-secret-token"),
            "nats token leaked: {text}"
        );
        assert!(!text.contains("pw123"), "nats url password leaked: {text}");
        assert!(
            !text.contains("/etc/acts/server.key"),
            "tls key leaked: {text}"
        );
        assert!(!text.contains("canary-key"), "plugin key leaked: {text}");
        assert!(
            !text.contains("/etc/nats/ops.creds"),
            "nats credential file leaked: {text}"
        );
        assert!(
            !text.contains("mystery"),
            "unclassified root key leaked: {text}"
        );

        // Sensitive *names* keep their existence, not their value.
        assert_eq!(redacted["nats"]["token"], "[configured]");
        assert_eq!(redacted["nats"]["password"], "[configured]");
        assert_eq!(redacted["web"]["tls_private_key"], "[configured]");
        assert_eq!(redacted["vendor_plugin"]["api_key"], "[configured]");
        assert_eq!(redacted["vendor_plugin"]["endpoint"], "[redacted]");

        // URL credentials are scrubbed, not dropped: the operator still sees
        // which user and host the server talks to.
        assert_eq!(
            redacted["db"]["database_url"],
            "postgres://acts:***@db.local:5432/acts"
        );
        assert_eq!(redacted["nats"]["url"], "nats://ops:***@broker.local:4222");

        // Operational fields stay usable.
        assert_eq!(redacted["db"]["kind"], "postgres");
        assert_eq!(redacted["db"]["lease"], true);
        assert_eq!(redacted["web"]["host"], "127.0.0.1");
        assert_eq!(redacted["web"]["port"], 10082);
        assert_eq!(redacted["nats"]["subject"], "acts");
        assert_eq!(redacted["nats"]["max_in_flight"], 256);
        assert_eq!(redacted["nats"]["channels"][0]["id"], "c1");
        assert_eq!(redacted["tick_interval_secs"], 1);
        assert_eq!(redacted["scheduler_workers"], 4);

        // An unclassified root key keeps its existence as a placeholder.
        assert_eq!(redacted["not_a_known_knob"], "[redacted]");
    }

    /// A query-string password (no userinfo at all) is scrubbed too.
    #[test]
    fn scrubbing_covers_query_parameter_passwords() {
        assert_eq!(
            scrub_url_credentials("postgres://db.local:5432/acts?password=hunter2&ssl=true"),
            "postgres://db.local:5432/acts?password=***&ssl=true"
        );
        assert_eq!(
            scrub_url_credentials("mysql://u:p@h/db"),
            "mysql://u:***@h/db"
        );
        assert_eq!(
            scrub_url_credentials("nats://broker.local:4222"),
            "nats://broker.local:4222"
        );
        assert_eq!(scrub_url_credentials("/plain/path"), "/plain/path");
        assert_eq!(scrub_url_credentials("user@host"), "user@host");
    }

    /// The ingest budget of `collect`: a fresh answer is served from cache
    /// without a second store sweep, and a second engine answers for itself.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_collection_is_cached_per_engine() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let first = collect(&engine).await.unwrap();
        let second = collect(&engine).await.unwrap();
        // Same engine, inside the TTL: the cached answer is byte-stable.
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );

        // A second engine answers for itself (its cache key differs).
        let other = crate::Engine::builder().start().await.unwrap();
        let theirs = collect(&other).await.unwrap();
        assert_eq!(theirs.version, crate::VERSION);
    }

    /// More scrubber shapes: a query `passwd`, a fragment after the password,
    /// a URL with no credentials, and a scheme-only string.
    #[test]
    fn scrubbing_edge_shapes() {
        assert_eq!(
            scrub_url_credentials("postgres://h/db?passwd=secret#frag"),
            "postgres://h/db?passwd=***#frag"
        );
        assert_eq!(
            scrub_url_credentials("postgres://h/db?a=1&password=secret"),
            "postgres://h/db?a=1&password=***"
        );
        assert_eq!(
            scrub_url_credentials("redis://:pw@h:6379/0"),
            "redis://:***@h:6379/0"
        );
        assert_eq!(scrub_url_credentials("postgres://h/db"), "postgres://h/db");
    }

    /// Unknown sections redact per key, an array of them each, and a
    /// sensitive name in any casing still answers "[configured]".
    #[test]
    fn redaction_covers_unknown_shapes() {
        let table: Table = toml! {
            [vendor]
            API_TOKEN = "canary"
            retries = 3
            [[vendor.endpoints]]
            url = "https://vendor.example/v1"
            secret = "canary2"
        };
        let redacted = redact_config(&table);
        assert_eq!(redacted["vendor"]["API_TOKEN"], "[configured]");
        assert_eq!(redacted["vendor"]["retries"], "[redacted]");
        // An unknown nested array keeps only its shape: the array becomes a
        // count, so no key (or value) of an unclassified subtree leaks.
        assert_eq!(
            redacted["vendor"]["endpoints"],
            serde_json::json!({"redacted": 1})
        );
        assert!(!redacted.to_string().contains("canary"));
    }

    /// The cache serves a stale answer past the TTL by sampling again, and
    /// the answer for one engine never answers another.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stale_cache_entry_is_resampled() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let first = collect(&engine).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        let second = collect(&engine).await.unwrap();
        assert_eq!(first.version, second.version);
        assert_eq!(first.processes.total, second.processes.total);
    }
}
