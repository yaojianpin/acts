//! Ingress budgets shared by every external transport (web, gRPC, NATS).
//!
//! The transports' own protocol layers bound what their wire formats can do
//! (HTTP body limits, gRPC message sizes, NATS broker payload caps). The
//! *business* budget every transport shares lives here, so a caller cannot
//! spend the engine through one transport what another refuses:
//!
//! - [`Profile`] — per-action byte/deadline/inflight budget, resolved from
//!   the `[ingress]` config section over built-in per-class defaults.
//! - [`Ingress::try_enter_action`] — the global and per-action inflight
//!   permits a transport must hold across one action dispatch.
//! - [`Ingress::try_enter_parse`] — the parse permit a transport holds while
//!   turning raw payload bytes into JSON structures.
//! - [`SubscriptionRegistry`] — the lease every SSE / gRPC subscription
//!   acquires before a queue is allocated, bounding total subscriptions
//!   across transports (per subject, per client id, per IP and creation
//!   rate).
//! - [`Metrics`] — the label counters (`acts_ingress_*`) reported by
//!   `sys:status`.
//!
//! Permits are try-acquire only: a refusal is answered to the caller (429 /
//! `resource_exhausted` / a NATS error reply) instead of queueing an
//! unbounded wait.

use crate::Config;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Transport label of the web (HTTP/SSE) plugin.
pub const TRANSPORT_WEB: &str = "web";
/// Transport label of the gRPC plugin.
pub const TRANSPORT_GRPC: &str = "grpc";
/// Transport label of the NATS plugin.
pub const TRANSPORT_NATS: &str = "nats";

/// The posture level: one knob that presets every ingress budget. A
/// section's explicit keys always win over the level; the level wins over
/// the built-in defaults.
///
/// - `permissive` — wide budgets for trusted internal traffic: payload caps
///   ×4, inflight ×2, global 4096, parse 256; the web/gRPC strict-TLS rule
///   is off (a non-loopback plaintext bind only warns).
/// - `standard` (default) — the documented defaults: global 1024, parse
///   128; a non-loopback bind needs TLS or an explicit
///   `plain_behind_proxy`.
/// - `strict` — tight budgets for exposed deployments: payload caps ÷4,
///   inflight ÷2, global 512, parse 64; a non-loopback bind needs TLS
///   (`plain_behind_proxy` is not enough).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    Permissive,
    #[default]
    Standard,
    Strict,
}

impl Level {
    /// Parse a config level string (case-insensitive); an unknown word is
    /// `None` (the caller warns and falls back to `Standard`).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "permissive" => Some(Level::Permissive),
            "standard" => Some(Level::Standard),
            "strict" => Some(Level::Strict),
            _ => None,
        }
    }

    /// The payload-cap factor of the level (permissive ×4, strict ÷4).
    fn bytes_mul(&self) -> u64 {
        match self {
            Level::Permissive => 4,
            Level::Standard => 1,
            Level::Strict => 1,
        }
    }

    fn bytes_div(&self) -> u64 {
        match self {
            Level::Strict => 4,
            _ => 1,
        }
    }

    fn inflight_mul(&self) -> u64 {
        match self {
            Level::Permissive => 2,
            _ => 1,
        }
    }

    fn inflight_div(&self) -> u64 {
        match self {
            Level::Strict => 2,
            _ => 1,
        }
    }

    /// The global inflight default of the level.
    fn global_default(&self) -> usize {
        match self {
            Level::Permissive => 4096,
            Level::Standard => 1024,
            Level::Strict => 512,
        }
    }

    /// The parse-permit default of the level.
    fn parse_default(&self) -> usize {
        match self {
            Level::Permissive => 256,
            Level::Standard => 128,
            Level::Strict => 64,
        }
    }

    /// Scale one budget by the level's factors, keeping the clamps honest
    /// (a scaled value re-clamps, so strict never dips under a usable
    /// floor and permissive never grows past a sane ceiling).
    fn scale_bytes(&self, default: usize) -> usize {
        let scaled = (default as u64 * self.bytes_mul() / self.bytes_div()) as usize;
        scaled.clamp(16 * 1024, 256 * 1024 * 1024)
    }

    fn scale_inflight(&self, default: usize) -> usize {
        let scaled = (default as u64 * self.inflight_mul() / self.inflight_div()) as usize;
        scaled.clamp(1, 65_536)
    }
}

/// The `[security]` section: the posture level every section inherits
/// unless it names its own.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecurityConfig {
    pub level: Option<String>,
}

/// Resolve the effective level of a section: its own `level` key, else the
/// `[security]` level, else `standard`.
pub fn resolve_level(section: Option<&str>, config: &Config) -> Level {
    if let Some(text) = section
        && let Some(level) = Level::parse(text)
    {
        return level;
    }
    let security: Option<SecurityConfig> = config.get("security").ok();
    if let Some(text) = security.and_then(|s| s.level)
        && let Some(level) = Level::parse(&text)
    {
        return level;
    }
    Level::Standard
}

/// The `[ingress]` config section.
///
/// Every field is optional; omitted fields follow the section's `level`
/// (which follows `[security].level`, which follows `standard`). The
/// `profile` table overrides a class or a single action by name and wins
/// over both the level and the defaults:
///
/// ```toml
/// [security]
/// level = "standard"        # permissive | standard | strict
///
/// [ingress]
/// # level = "strict"        # this section alone, tighter than the rest
/// # global_inflight = 1024   # explicit keys always win
/// # parse_inflight = 128
///
/// [ingress.profile.deploy]
/// max_raw_bytes = 16777216
/// deadline_ms = 60000
/// max_inflight = 4
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IngressConfig {
    /// The posture level of this section (`permissive`/`standard`/
    /// `strict`); omitted follows `[security].level`, then `standard`.
    pub level: Option<String>,
    /// Concurrent action dispatches across every transport.
    pub global_inflight: Option<usize>,
    /// Concurrent raw-payload parses across every transport.
    pub parse_inflight: Option<usize>,
    /// Per-class or per-action overrides, keyed by class name (`auth`,
    /// `small`, `event`, `snapshot`, `start`, `deploy`, `default`) or by
    /// exact action name (`model:deploy`).
    #[serde(default)]
    pub profile: Option<std::collections::HashMap<String, ProfileOverride>>,
}

/// One override row of `[ingress.profile]`. `None` fields keep the class
/// default.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ProfileOverride {
    pub max_raw_bytes: Option<usize>,
    pub deadline_ms: Option<u64>,
    pub max_inflight: Option<usize>,
}

/// Inflight limits clamped to something that still bounds the engine: a
/// value of `0` selects the default (mirroring `[nats].max_in_flight`).
fn clamped(value: Option<usize>, default: usize, max: usize) -> usize {
    match value {
        None => default,
        Some(0) => default,
        Some(v) => v.clamp(1, max),
    }
}

/// Byte budgets clamped to sane floors and ceilings: a body cap below 16 KiB
/// starves the small actions, one past 256 MiB is no cap at all.
fn clamped_bytes(value: Option<usize>, default: usize) -> usize {
    match value {
        None => default,
        Some(0) => default,
        Some(v) => v.clamp(16 * 1024, 256 * 1024 * 1024),
    }
}

/// Deadlines clamped to 1 s .. 1 h.
fn clamped_deadline(value: Option<u64>, default: u64) -> u64 {
    match value {
        None => default,
        Some(0) => default,
        Some(v) => v.clamp(1_000, 3_600_000),
    }
}

/// The resolved budget of one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// The class the action resolved to (`deploy`, `small`, …), or
    /// `default`.
    pub class: &'static str,
    /// Raw payload cap the transport enforces before decoding.
    pub max_raw_bytes: usize,
    /// Server-side deadline for one dispatch.
    pub deadline_ms: u64,
    /// Concurrent dispatches of this action.
    pub max_inflight: usize,
}

const CLASS_AUTH: &str = "auth";
const CLASS_SMALL: &str = "small";
const CLASS_EVENT: &str = "event";
const CLASS_SNAPSHOT: &str = "snapshot";
const CLASS_START: &str = "start";
const CLASS_DEPLOY: &str = "deploy";
const CLASS_DEFAULT: &str = "default";

/// Credential-carrying actions: the request body *is* the credential, so
/// their profile is tight on bytes but roomy enough for the password KDF.
const AUTH_ACTIONS: &[&str] = &["acl:login", "acl:refresh"];
/// Row reads and acks: tiny bodies, short deadlines, the widest inflight.
const SMALL_ACTIONS: &[&str] = &[
    "model:ls",
    "model:get",
    "model:rm",
    "pack:ls",
    "pack:get",
    "proc:ls",
    "proc:get",
    "proc:graph",
    "task:ls",
    "task:get",
    "msg:ls",
    "msg:get",
    "msg:ack",
    "evt:ls",
    "evt:get",
    "snap:get",
    "snap:ls",
    "msg:sub",
    "msg:unsub",
    "acl:whoami",
    "acl:getuser",
    "acl:users",
];
/// External entry points (`evt:start`, the HTTP `/hooks/{event_id}` route
/// under its `evt:start` name): attacker-shaped JSON, bounded harder.
const EVENT_ACTIONS: &[&str] = &["evt:start"];
/// Snapshot feed writes.
const SNAPSHOT_ACTIONS: &[&str] = &["snap:upsert", "snap:remove", "snap:reply"];
/// Process starts: inputs and vars, plus the model load and scheduler write
/// they trigger.
const START_ACTIONS: &[&str] = &["proc:start", "proc:start_from_model"];
/// Catalogue writes: the whole workflow source, expressions included.
const DEPLOY_ACTIONS: &[&str] = &["model:deploy", "pack:publish"];

const AUTH_PROFILE: Profile = Profile {
    class: CLASS_AUTH,
    max_raw_bytes: 64 * 1024,
    deadline_ms: 10_000,
    max_inflight: 32,
};
const SMALL_PROFILE: Profile = Profile {
    class: CLASS_SMALL,
    max_raw_bytes: 64 * 1024,
    deadline_ms: 5_000,
    max_inflight: 128,
};
const EVENT_PROFILE: Profile = Profile {
    class: CLASS_EVENT,
    max_raw_bytes: 1024 * 1024,
    deadline_ms: 15_000,
    max_inflight: 64,
};
const SNAPSHOT_PROFILE: Profile = Profile {
    class: CLASS_SNAPSHOT,
    max_raw_bytes: 1024 * 1024,
    deadline_ms: 15_000,
    max_inflight: 32,
};
const START_PROFILE: Profile = Profile {
    class: CLASS_START,
    max_raw_bytes: 2 * 1024 * 1024,
    deadline_ms: 30_000,
    max_inflight: 32,
};
const DEPLOY_PROFILE: Profile = Profile {
    class: CLASS_DEPLOY,
    max_raw_bytes: 8 * 1024 * 1024,
    deadline_ms: 60_000,
    max_inflight: 4,
};
const DEFAULT_PROFILE: Profile = Profile {
    class: CLASS_DEFAULT,
    max_raw_bytes: 256 * 1024,
    deadline_ms: 30_000,
    max_inflight: 64,
};

/// The class an action belongs to; unknown actions are `default`.
pub fn class_of(action: &str) -> &'static str {
    fn listed(action: &str, actions: &[&str]) -> bool {
        actions.contains(&action)
    }
    if listed(action, AUTH_ACTIONS) {
        CLASS_AUTH
    } else if listed(action, SMALL_ACTIONS) {
        CLASS_SMALL
    } else if listed(action, EVENT_ACTIONS) {
        CLASS_EVENT
    } else if listed(action, SNAPSHOT_ACTIONS) {
        CLASS_SNAPSHOT
    } else if listed(action, START_ACTIONS) {
        CLASS_START
    } else if listed(action, DEPLOY_ACTIONS) {
        CLASS_DEPLOY
    } else {
        CLASS_DEFAULT
    }
}

fn class_profile(class: &str) -> Profile {
    match class {
        CLASS_AUTH => AUTH_PROFILE,
        CLASS_SMALL => SMALL_PROFILE,
        CLASS_EVENT => EVENT_PROFILE,
        CLASS_SNAPSHOT => SNAPSHOT_PROFILE,
        CLASS_START => START_PROFILE,
        CLASS_DEPLOY => DEPLOY_PROFILE,
        _ => DEFAULT_PROFILE,
    }
}

/// The bound a try-acquire hit, for the transport's rejection answer and its
/// `acts_ingress_rejected_total` metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionReject {
    /// The engine-wide inflight cap is full.
    GlobalFull,
    /// The action's own inflight cap is full.
    ActionFull { action: String },
    /// The parse-permit pool is full (parse pressure, not action pressure).
    ParseFull,
}

impl AdmissionReject {
    /// The stable external code the transport answers with — the same code
    /// on every transport, so a caller's retry logic is transport-agnostic.
    pub fn code(&self) -> &'static str {
        match self {
            AdmissionReject::GlobalFull
            | AdmissionReject::ActionFull { .. }
            | AdmissionReject::ParseFull => "resource_exhausted",
        }
    }

    /// The metric label for the rejection reason.
    pub fn reason(&self) -> &'static str {
        match self {
            AdmissionReject::GlobalFull => "global_inflight",
            AdmissionReject::ActionFull { .. } => "action_inflight",
            AdmissionReject::ParseFull => "parse_pressure",
        }
    }
}

/// One action dispatch's inflight permit. Held across the whole dispatch
/// (`apply_as` and its effect waits); dropped releases both the global and
/// the per-action slot.
pub struct ActionPermit {
    _global: OwnedSemaphorePermit,
    _action: OwnedSemaphorePermit,
}

/// One raw-payload parse's permit. The permit itself is never inspected —
/// it exists so a dropped `ParsePermit` releases the parse slot.
pub struct ParsePermit {
    /// Held for RAII only; never read.
    _permit: OwnedSemaphorePermit,
}

/// Label counters, `name{k=v,...} -> value`. Reported by `sys:status` under
/// `metrics`.
#[derive(Default)]
pub struct Metrics {
    counters: DashMap<String, AtomicU64>,
}

impl Metrics {
    /// Increment one counter by one.
    pub fn incr(&self, name: &str, labels: &[(&str, &str)]) {
        let key = metric_key(name, labels);
        if let Some(entry) = self.counters.get_mut(&key) {
            entry.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.entry(key).or_insert(AtomicU64::new(1));
        }
    }

    /// Increment one counter by `delta`.
    pub fn incr_by(&self, name: &str, labels: &[(&str, &str)], delta: u64) {
        let key = metric_key(name, labels);
        if let Some(entry) = self.counters.get_mut(&key) {
            entry.fetch_add(delta, Ordering::Relaxed);
        } else {
            self.counters.entry(key).or_insert(AtomicU64::new(delta));
        }
    }

    /// A sorted snapshot of every counter (a `BTreeMap` so the status answer
    /// is stable across calls).
    pub fn snapshot(&self) -> BTreeMap<String, u64> {
        self.counters
            .iter()
            .map(|entry| (entry.key().clone(), entry.load(Ordering::Relaxed)))
            .collect()
    }
}

/// `name{k=v,...}`, labels sorted so a metric's key is stable regardless of
/// the call site's label order.
fn metric_key(name: &str, labels: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(&str, &str)> = labels.to_vec();
    sorted.sort_unstable();
    let inner = sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{name}{{{inner}}}")
}

/// One live subscription lease: SSE and gRPC acquire it *before* allocating
/// their queue and registering the channel handler; dropping it releases
/// every counter it took.
pub struct SubscriptionLease {
    registry: Arc<SubscriptionRegistry>,
    key: SubscriptionKey,
}

impl std::fmt::Debug for SubscriptionLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionLease")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl Drop for SubscriptionLease {
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

/// Why a subscription lease was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionReject {
    /// The engine-wide subscription count is at its cap.
    GlobalFull,
    /// The subject's subscription count is at its cap (across transports).
    SubjectFull,
    /// The caller IP's subscription count is at its cap.
    IpFull,
    /// The same `(subject, client_id)` already holds a live lease: a second
    /// connection must not double-ack the same deliveries.
    ClientBusy,
    /// The subject's subscription creation rate is exhausted (connect
    /// flapping).
    RateLimited,
}

impl SubscriptionReject {
    pub fn reason(&self) -> &'static str {
        match self {
            SubscriptionReject::GlobalFull => "global_full",
            SubscriptionReject::SubjectFull => "subject_full",
            SubscriptionReject::IpFull => "ip_full",
            SubscriptionReject::ClientBusy => "client_busy",
            SubscriptionReject::RateLimited => "rate_limited",
        }
    }
}

/// The identity a subscription acquires a lease under.
#[derive(Debug, Clone)]
pub struct SubscriptionKey {
    /// Which transport the subscription arrived on (`web`, `grpc`).
    pub transport: &'static str,
    /// The channel key the `msg:sub` action answered (subject + client id).
    pub subject: String,
    /// The subscriber's self-declared client id. An empty id cannot claim a
    /// per-client slot (there is nothing stable to dedupe on), so it is
    /// counted only in the global/subject/IP buckets.
    pub client_id: String,
    /// The subscriber's remote address, when the transport knows it.
    pub ip: Option<IpAddr>,
}

/// Cross-transport subscription quota (`[subscriptions]` section).
pub struct SubscriptionRegistry {
    global_max: usize,
    per_subject: usize,
    per_ip: usize,
    per_client: usize,
    rate_per_sec: u64,
    global: AtomicUsize,
    by_subject: DashMap<String, usize>,
    by_ip: DashMap<IpAddr, usize>,
    /// Live lease count per transport label, for the gauges.
    by_transport: DashMap<&'static str, AtomicUsize>,
    /// `(subject, client_id) -> live count` — `per_client` is 1, so a busy
    /// pair refuses the second connection.
    by_client: DashMap<(String, String), usize>,
    /// Per-subject fixed one-second creation windows: `(window_start_ms,
    /// count)`.
    windows: DashMap<String, (u64, AtomicU64)>,
}

/// The `[subscriptions]` config section: the posture level presets the
/// caps, explicit keys win.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubscriptionLimitsConfig {
    /// The posture level of this section; omitted follows
    /// `[security].level`, then `standard`.
    pub level: Option<String>,
    pub global_max: Option<usize>,
    pub per_subject: Option<usize>,
    pub per_ip: Option<usize>,
    pub per_client: Option<usize>,
    pub create_rate_per_sec: Option<u64>,
}

const SUB_GLOBAL_DEFAULT: usize = 10_000;
const SUB_PER_SUBJECT_DEFAULT: usize = 32;
const SUB_PER_IP_DEFAULT: usize = 128;
const SUB_PER_CLIENT_DEFAULT: usize = 1;
const SUB_RATE_DEFAULT: u64 = 20;

/// The per-level subscription caps `(global, subject, ip, rate)`; the
/// per-client cap is 1 at every level.
fn subscription_defaults(level: Level) -> (usize, usize, usize, u64) {
    match level {
        Level::Permissive => (20_000, 64, 256, 40),
        Level::Standard => (
            SUB_GLOBAL_DEFAULT,
            SUB_PER_SUBJECT_DEFAULT,
            SUB_PER_IP_DEFAULT,
            SUB_RATE_DEFAULT,
        ),
        Level::Strict => (2_000, 16, 64, 10),
    }
}

impl SubscriptionRegistry {
    fn new(config: Option<&SubscriptionLimitsConfig>, level: Level) -> Arc<Self> {
        let config = config.cloned().unwrap_or_default();
        let (global_d, subject_d, ip_d, rate_d) = subscription_defaults(level);
        let clamp = |value: Option<usize>, default: usize, max: usize| match value {
            None => default,
            Some(0) => default,
            Some(v) => v.clamp(1, max),
        };
        Arc::new(Self {
            global_max: clamp(config.global_max, global_d, 1_000_000),
            per_subject: clamp(config.per_subject, subject_d, 100_000),
            per_ip: clamp(config.per_ip, ip_d, 1_000_000),
            per_client: clamp(config.per_client, SUB_PER_CLIENT_DEFAULT, 1024),
            rate_per_sec: match config.create_rate_per_sec {
                None => rate_d,
                Some(0) => rate_d,
                Some(v) => v.clamp(1, 100_000),
            },
            global: AtomicUsize::new(0),
            by_subject: DashMap::new(),
            by_ip: DashMap::new(),
            by_transport: DashMap::new(),
            by_client: DashMap::new(),
            windows: DashMap::new(),
        })
    }

    /// Acquire one lease, or the reason it was refused. The check-and-take
    /// of each bucket runs under the bucket's own shard lock
    /// (`DashMap::entry`), so two connections racing on the last slot cannot
    /// both take it.
    pub fn try_acquire(
        self: &Arc<Self>,
        key: SubscriptionKey,
    ) -> Result<SubscriptionLease, SubscriptionReject> {
        // Creation rate first: connect flapping must not even consume quota
        // slots it will drop a second later.
        let now = unix_millis();
        {
            let mut window = self
                .windows
                .entry(key.subject.clone())
                .or_insert_with(|| (now, AtomicU64::new(0)));
            if now.saturating_sub(window.0) >= 1_000 {
                window.0 = now;
                window.1.store(0, Ordering::Relaxed);
            }
            if window.1.fetch_add(1, Ordering::Relaxed) >= self.rate_per_sec {
                return Err(SubscriptionReject::RateLimited);
            }
        }

        if self.global.load(Ordering::Relaxed) >= self.global_max {
            return Err(SubscriptionReject::GlobalFull);
        }
        {
            let mut count = self.by_subject.entry(key.subject.clone()).or_insert(0);
            if *count >= self.per_subject {
                return Err(SubscriptionReject::SubjectFull);
            }
            *count += 1;
        }
        if let Some(ip) = key.ip {
            let mut count = self.by_ip.entry(ip).or_insert(0);
            if *count >= self.per_ip {
                drop(count);
                self.entry_undo(&key.subject);
                return Err(SubscriptionReject::IpFull);
            }
            *count += 1;
            drop(count);
        }
        if !key.client_id.is_empty() {
            let mut count = self
                .by_client
                .entry((key.subject.clone(), key.client_id.clone()))
                .or_insert(0);
            if *count >= self.per_client {
                drop(count);
                self.entry_undo(&key.subject);
                if let Some(ip) = key.ip {
                    self.ip_undo(ip);
                }
                return Err(SubscriptionReject::ClientBusy);
            }
            *count += 1;
        }

        self.global.fetch_add(1, Ordering::Relaxed);
        if let Some(counter) = self.by_transport.get(key.transport) {
            counter.fetch_add(1, Ordering::Relaxed);
        } else {
            self.by_transport
                .entry(key.transport)
                .or_insert_with(|| AtomicUsize::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(SubscriptionLease {
            registry: self.clone(),
            key,
        })
    }

    fn entry_undo(&self, subject: &str) {
        if let Some(mut count) = self.by_subject.get_mut(subject) {
            *count -= 1;
        }
    }

    fn ip_undo(&self, ip: IpAddr) {
        if let Some(mut count) = self.by_ip.get_mut(&ip) {
            *count -= 1;
        }
    }

    fn release(&self, key: &SubscriptionKey) {
        self.global.fetch_sub(1, Ordering::Relaxed);
        if let Some(counter) = self.by_transport.get(key.transport) {
            counter.fetch_sub(1, Ordering::Relaxed);
        }
        if let Some(mut count) = self.by_subject.get_mut(&key.subject) {
            *count -= 1;
        }
        if let Some(ip) = key.ip
            && let Some(mut count) = self.by_ip.get_mut(&ip)
        {
            *count -= 1;
        }
        if !key.client_id.is_empty()
            && let Some(mut count) = self
                .by_client
                .get_mut(&(key.subject.clone(), key.client_id.clone()))
        {
            *count -= 1;
        }
    }

    /// Live subscription count.
    pub fn len(&self) -> usize {
        self.global.load(Ordering::Relaxed)
    }

    /// Whether no subscription is live.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `acts_subscriptions{transport}` gauges for `sys:status`, plus the
    /// engine-wide total.
    pub fn gauges(&self) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = self
            .by_transport
            .iter()
            .map(|entry| {
                (
                    metric_key("acts_subscriptions", &[("transport", *entry.key())]),
                    entry.value().load(Ordering::Relaxed) as u64,
                )
            })
            .collect();
        out.insert("acts_subscriptions_total".to_string(), self.len() as u64);
        out
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The engine's shared ingress budget. Built once per engine from the
/// `[ingress]` and `[subscriptions]` config sections; every transport talks
/// to the same instance.
pub struct Ingress {
    config: IngressConfig,
    /// The effective posture level of the `[ingress]` section.
    level: Level,
    global: Arc<Semaphore>,
    parse: Arc<Semaphore>,
    /// Per-action semaphores, created on first use at the profile's cap.
    actions: DashMap<String, Arc<Semaphore>>,
    pub subscriptions: Arc<SubscriptionRegistry>,
    pub metrics: Metrics,
}

impl Ingress {
    /// Build from the engine config: the section's `level` (following
    /// `[security].level`) presets the budgets, explicit keys win.
    pub fn new(config: &Config) -> Arc<Self> {
        let ingress_config: IngressConfig = config.get("ingress").unwrap_or_default();
        let level = resolve_level(ingress_config.level.as_deref(), config);
        if let Some(text) = ingress_config.level.as_deref()
            && Level::parse(text).is_none()
        {
            tracing::warn!(
                level = %text,
                "unknown ingress level; using 'standard' (permissive | standard | strict)"
            );
        }
        // A profile key is either a class name or an exact action name
        // (which contains ':'). An unknown class name still applies to
        // nothing and silently changes no budget, so say so.
        if let Some(overrides) = ingress_config.profile.as_ref() {
            for key in overrides.keys() {
                if !key.contains(':')
                    && key != CLASS_DEFAULT
                    && class_profile(key).class == CLASS_DEFAULT
                {
                    tracing::warn!(
                        profile = %key,
                        "unknown ingress profile class; the row applies only to actions literally named '{key}'"
                    );
                }
            }
        }
        let subscriptions: Option<SubscriptionLimitsConfig> = config.get("subscriptions").ok();
        Arc::new(Self {
            global: Arc::new(Semaphore::new(match ingress_config.global_inflight {
                None => level.global_default(),
                Some(0) => level.global_default(),
                Some(v) => clamped(Some(v), level.global_default(), 1_048_576),
            })),
            parse: Arc::new(Semaphore::new(match ingress_config.parse_inflight {
                None => level.parse_default(),
                Some(0) => level.parse_default(),
                Some(v) => clamped(Some(v), level.parse_default(), 65_536),
            })),
            config: ingress_config,
            level,
            actions: DashMap::new(),
            subscriptions: SubscriptionRegistry::new(
                subscriptions.as_ref(),
                resolve_level(
                    subscriptions.as_ref().and_then(|s| s.level.as_deref()),
                    config,
                ),
            ),
            metrics: Metrics::default(),
        })
    }

    /// The budget of one action: its class default scaled by the section's
    /// level, merged with any `[ingress.profile]` override (by class name
    /// or exact action name — overrides win over both level and defaults).
    pub fn profile(&self, action: &str) -> Profile {
        let class = class_of(action);
        let base = class_profile(class);
        let mut profile = Profile {
            max_raw_bytes: self.level.scale_bytes(base.max_raw_bytes),
            deadline_ms: base.deadline_ms,
            max_inflight: self.level.scale_inflight(base.max_inflight),
            class: base.class,
        };
        let overrides = self.config.profile.as_ref();
        let Some(overrides) = overrides else {
            return profile;
        };
        // Class override first, exact-action override second (the more
        // specific key wins, applied last).
        if let Some(row) = overrides.get(class) {
            profile.apply(row);
        }
        if let Some(row) = overrides.get(action) {
            profile.apply(row);
            profile.class = class;
        }
        profile
    }

    /// The effective posture level of this engine's ingress (`permissive`,
    /// `standard`, `strict`) — what the transports' strict-TLS and other
    /// posture rules follow.
    pub fn level(&self) -> Level {
        self.level
    }

    /// Acquire one action dispatch's inflight permits (global + per-action),
    /// or the bound that refused it.
    pub fn try_enter_action(&self, action: &str) -> Result<ActionPermit, AdmissionReject> {
        let profile = self.profile(action);
        let global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| AdmissionReject::GlobalFull)?;
        let semaphore = self
            .actions
            .entry(action.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(profile.max_inflight)))
            .clone();
        let action_permit =
            semaphore
                .clone()
                .try_acquire_owned()
                .map_err(|_| AdmissionReject::ActionFull {
                    action: action.to_string(),
                })?;
        Ok(ActionPermit {
            _global: global,
            _action: action_permit,
        })
    }

    /// Acquire one parse permit, or `None` under parse pressure.
    pub fn try_enter_parse(&self) -> Option<ParsePermit> {
        self.parse
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|_permit| ParsePermit { _permit })
    }

    /// Live global action inflight count.
    pub fn action_inflight(&self) -> usize {
        self.global.available_permits()
    }
}

impl Profile {
    fn apply(&mut self, row: &ProfileOverride) {
        if let Some(bytes) = row.max_raw_bytes {
            self.max_raw_bytes = clamped_bytes(Some(bytes), self.max_raw_bytes);
        }
        if let Some(deadline) = row.deadline_ms {
            self.deadline_ms = clamped_deadline(Some(deadline), self.deadline_ms);
        }
        if let Some(inflight) = row.max_inflight {
            self.max_inflight = clamped(Some(inflight), self.max_inflight, 65_536);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn ingress_with(table: toml::Table) -> Arc<Ingress> {
        let config = Config {
            data: Default::default(),
            table,
        };
        Ingress::new(&config)
    }

    /// Every action resolves to a profile; the class table routes the known
    /// names and everything else lands on `default`.
    #[test]
    fn profiles_resolve_by_class() {
        let ingress = ingress_with(toml::Table::new());
        assert_eq!(ingress.profile("acl:login").class, "auth");
        assert_eq!(ingress.profile("model:deploy").class, "deploy");
        assert_eq!(
            ingress.profile("model:deploy").max_raw_bytes,
            8 * 1024 * 1024
        );
        assert_eq!(ingress.profile("proc:start").class, "start");
        assert_eq!(ingress.profile("snap:upsert").class, "snapshot");
        assert_eq!(ingress.profile("evt:start").class, "event");
        assert_eq!(ingress.profile("msg:ack").class, "small");
        assert_eq!(ingress.profile("totally:unknown").class, "default");
    }

    /// A class override moves every action in the class; an exact-action
    /// override wins over the class one.
    #[test]
    fn profile_overrides_apply_class_then_action() {
        let mut table = toml::Table::new();
        let value: toml::Value =
            toml::from_str("[deploy]\nmax_raw_bytes = 33554432\ndeadline_ms = 90000\n\n[\"model:deploy\"]\nmax_inflight = 2\n").unwrap();
        table.insert("profile".to_string(), value);
        let ingress = ingress_with(toml::Table::from_iter([(
            "ingress".to_string(),
            toml::Value::Table(table),
        )]));

        let deploy = ingress.profile("model:deploy");
        assert_eq!(deploy.max_raw_bytes, 32 * 1024 * 1024);
        assert_eq!(deploy.deadline_ms, 90_000);
        assert_eq!(deploy.max_inflight, 2);
        // The class override alone moves the sibling too.
        let publish = ingress.profile("pack:publish");
        assert_eq!(publish.max_raw_bytes, 32 * 1024 * 1024);
    }

    /// Admission takes global and per-action slots; past the action cap the
    /// refusal names the action, and dropped permits free the slots.
    #[tokio::test]
    async fn action_admission_is_bounded_and_released() {
        let mut table = toml::Table::new();
        let value: toml::Value =
            toml::from_str("[profile]\n\n[profile.\"msg:ack\"]\nmax_inflight = 1\n").unwrap();
        table.insert("ingress".to_string(), value);
        let ingress = ingress_with(table);

        let first = ingress.try_enter_action("msg:ack").expect("first admitted");
        let rejected = ingress.try_enter_action("msg:ack");
        assert!(
            matches!(&rejected, Err(AdmissionReject::ActionFull { action }) if action == "msg:ack"),
            "the second dispatch must be refused on the action cap"
        );
        drop(first);
        assert!(
            ingress.try_enter_action("msg:ack").is_ok(),
            "a finished dispatch frees its slot"
        );
    }

    /// The global cap refuses dispatches whose action cap still has room.
    #[tokio::test]
    async fn global_cap_refuses_before_the_action_cap_matters() {
        let mut table = toml::Table::new();
        let value: toml::Value = toml::from_str("global_inflight = 1\n").unwrap();
        table.insert("ingress".to_string(), value);
        let ingress = ingress_with(table);

        let _first = ingress.try_enter_action("msg:ack").expect("first admitted");
        assert!(
            matches!(
                ingress.try_enter_action("model:ls"),
                Err(AdmissionReject::GlobalFull)
            ),
            "the global cap refuses every other action"
        );
    }

    /// Parse permits are a separate, smaller pool: parse pressure refuses
    /// parsing without touching the action budget.
    #[tokio::test]
    async fn parse_permits_are_independent() {
        let mut table = toml::Table::new();
        let value: toml::Value = toml::from_str("parse_inflight = 1\n").unwrap();
        table.insert("ingress".to_string(), value);
        let ingress = ingress_with(table);

        let _first = ingress.try_enter_parse().expect("first parse admitted");
        assert!(
            ingress.try_enter_parse().is_none(),
            "a second concurrent parse is refused"
        );
        assert!(
            ingress.try_enter_action("msg:ack").is_ok(),
            "parse pressure must not consume the action budget"
        );
    }

    /// Subscriptions: the global, per-subject, per-IP and per-client buckets
    /// all bound; the lease release frees every bucket it took; and the
    /// same `(subject, client_id)` cannot hold two leases.
    #[test]
    fn subscription_registry_bounds_every_bucket() {
        let mut table = toml::Table::new();
        let value: toml::Value = toml::from_str(
            "global_max = 6\nper_subject = 4\nper_ip = 1\nper_client = 1\ncreate_rate_per_sec = 100\n",
        )
        .unwrap();
        table.insert("subscriptions".to_string(), value);
        let config = Config {
            data: Default::default(),
            table,
        };
        let ingress = Ingress::new(&config);
        let registry = ingress.subscriptions.clone();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();

        let key = |client: &str| SubscriptionKey {
            transport: TRANSPORT_WEB,
            subject: "subj".to_string(),
            client_id: client.to_string(),
            ip: Some(ip),
        };
        let bare = |subject: &str, client: &str| SubscriptionKey {
            transport: TRANSPORT_WEB,
            subject: subject.to_string(),
            client_id: client.to_string(),
            ip: None,
        };

        let a = registry.try_acquire(key("a")).expect("first admitted");
        // per_ip = 1: the second subscription from the same IP is refused
        // even though the subject and global caps still have room.
        assert_eq!(
            registry.try_acquire(key("b")).err(),
            Some(SubscriptionReject::IpFull),
        );
        drop(a);
        let a = registry
            .try_acquire(key("a"))
            .expect("readmitted after release");
        let b = registry
            .try_acquire(bare("subj", "b"))
            .expect("another subscriber admitted");
        // The same (subject, client id) from a different transport face
        // (`ip: None` — as if another transport) must not hold two leases:
        // two connections would ack the same deliveries.
        assert_eq!(
            registry.try_acquire(bare("subj", "a")).err(),
            Some(SubscriptionReject::ClientBusy),
        );
        let _c = registry
            .try_acquire(bare("subj", "c"))
            .expect("third admitted");
        let _d = registry
            .try_acquire(bare("subj", "d"))
            .expect("fourth admitted");
        assert_eq!(
            registry.try_acquire(bare("subj", "e")).err(),
            Some(SubscriptionReject::SubjectFull),
            "the subject cap (4) refuses the fifth subscription"
        );
        drop(a);
        drop(b);

        // global_max = 6: with `subj` holding 2, four more distinct
        // subjects fill the engine cap and the sixth new subject refuses.
        let mut held = Vec::new();
        for i in 0..4 {
            held.push(
                registry
                    .try_acquire(bare(&format!("h{i}"), &format!("c{i}")))
                    .expect("within the global cap"),
            );
        }
        assert_eq!(
            registry.try_acquire(bare("h9", "c9")).err(),
            Some(SubscriptionReject::GlobalFull),
        );
        drop(held);
        drop(_c);
        drop(_d);
        assert!(registry.is_empty(), "dropped leases release every bucket");
    }

    /// The creation-rate window refuses connect flapping on one subject.
    #[test]
    fn subscription_creation_is_rate_limited() {
        let mut table = toml::Table::new();
        let value: toml::Value = toml::from_str(
            "global_max = 10000\nper_subject = 100\nper_ip = 100\ncreate_rate_per_sec = 2\n",
        )
        .unwrap();
        table.insert("subscriptions".to_string(), value);
        let config = Config {
            data: Default::default(),
            table,
        };
        let registry = Ingress::new(&config).subscriptions.clone();

        let key = |client: &str| SubscriptionKey {
            transport: TRANSPORT_GRPC,
            subject: "flap".to_string(),
            client_id: client.to_string(),
            ip: None,
        };
        let a = registry.try_acquire(key("a")).expect("first admitted");
        let b = registry.try_acquire(key("b")).expect("second admitted");
        assert_eq!(
            registry.try_acquire(key("c")).err(),
            Some(SubscriptionReject::RateLimited),
            "the third creation in the same window is refused"
        );
        drop(a);
        drop(b);
    }

    /// The metric key is label-order independent.
    #[test]
    fn metric_keys_are_label_order_independent() {
        let a = metric_key(
            "acts_ingress_requests_total",
            &[
                ("transport", "web"),
                ("action", "proc:start"),
                ("result", "ok"),
            ],
        );
        let b = metric_key(
            "acts_ingress_requests_total",
            &[
                ("result", "ok"),
                ("action", "proc:start"),
                ("transport", "web"),
            ],
        );
        assert_eq!(a, b);
    }

    /// One level presets every budget: `strict` shrinks the deploy and
    /// global caps below `standard`, `permissive` grows them — and an
    /// explicit key or profile override beats the level.
    #[test]
    fn level_presets_budgets_and_explicit_keys_win() {
        let ingress_at = |body: &str| ingress_with(toml::from_str::<toml::Table>(body).unwrap());

        let strict = ingress_at("[ingress]\nlevel = \"strict\"\n");
        assert_eq!(strict.level(), Level::Strict);
        assert_eq!(
            strict.profile("model:deploy").max_raw_bytes,
            2 * 1024 * 1024
        );
        assert_eq!(strict.profile("msg:ack").max_raw_bytes, 16 * 1024);
        assert!(
            strict.profile("model:deploy").max_inflight
                < ingress_with(toml::Table::new())
                    .profile("model:deploy")
                    .max_inflight,
            "strict inflight is below standard"
        );

        let permissive = ingress_at("[ingress]\nlevel = \"permissive\"\n");
        assert_eq!(
            permissive.profile("model:deploy").max_raw_bytes,
            32 * 1024 * 1024
        );
        assert!(
            permissive.profile("model:deploy").max_inflight
                > ingress_with(toml::Table::new())
                    .profile("model:deploy")
                    .max_inflight,
            "permissive inflight is above standard"
        );

        // An explicit key wins over the level's preset: exactly ten
        // dispatches fit, the eleventh refuses on the global cap.
        let explicit = ingress_at("[ingress]\nlevel = \"permissive\"\nglobal_inflight = 10\n");
        let mut held = Vec::new();
        for _ in 0..10 {
            held.push(
                explicit
                    .try_enter_action("msg:ack")
                    .expect("within the explicit global cap"),
            );
        }
        assert!(
            matches!(
                explicit.try_enter_action("model:ls"),
                Err(AdmissionReject::GlobalFull)
            ),
            "the explicit global_inflight = 10 beats the permissive preset"
        );
        drop(held);

        // A profile override wins over the level, too.
        let overridden = ingress_at(
            "[ingress]\nlevel = \"strict\"\n\n[ingress.profile.deploy]\nmax_raw_bytes = 16777216\n",
        );
        assert_eq!(
            overridden.profile("model:deploy").max_raw_bytes,
            16 * 1024 * 1024
        );

        // An unknown level falls back to standard.
        let unknown = ingress_at("[ingress]\nlevel = \"yolo\"\n");
        assert_eq!(unknown.level(), Level::Standard);
    }

    /// `[security].level` is the default every section inherits; a
    /// section's own `level` wins over it — and the subscription registry
    /// follows the same rule.
    #[test]
    fn security_level_is_the_inherited_default() {
        let config_of = |body: &str| {
            let table: toml::Table = toml::from_str(body).unwrap();
            Config {
                data: Default::default(),
                table,
            }
        };

        let inherited = Ingress::new(&config_of("[security]\nlevel = \"strict\"\n"));
        assert_eq!(inherited.level(), Level::Strict);
        assert_eq!(
            inherited.profile("model:deploy").max_raw_bytes,
            2 * 1024 * 1024
        );

        let overridden = Ingress::new(&config_of(
            "[security]\nlevel = \"strict\"\n\n[ingress]\nlevel = \"permissive\"\n",
        ));
        assert_eq!(overridden.level(), Level::Permissive);

        // Subscriptions inherit the security level: strict presets 2000
        // global / 16 per subject.
        let subs = Ingress::new(&config_of(
            "[security]\nlevel = \"strict\"\n\n[subscriptions]\ncreate_rate_per_sec = 1000\n",
        ))
        .subscriptions
        .clone();
        let key = |client: &str| SubscriptionKey {
            transport: TRANSPORT_WEB,
            subject: "s".to_string(),
            client_id: client.to_string(),
            ip: None,
        };
        let mut held = Vec::new();
        for i in 0..16 {
            held.push(
                subs.try_acquire(key(&format!("u{i}")))
                    .expect("within the strict subject cap"),
            );
        }
        assert_eq!(
            subs.try_acquire(key("u16")).err(),
            Some(SubscriptionReject::SubjectFull),
            "the strict per-subject preset (16) refuses the 17th"
        );
        drop(held);

        // An explicit subscriptions key still beats the level.
        let subs = Ingress::new(&config_of(
            "[security]\nlevel = \"strict\"\n\n[subscriptions]\nper_subject = 1\n",
        ))
        .subscriptions
        .clone();
        let a = subs.try_acquire(key("a")).expect("admitted");
        assert_eq!(
            subs.try_acquire(key("b")).err(),
            Some(SubscriptionReject::SubjectFull),
            "the explicit per_subject = 1 beats the strict preset"
        );
        drop(a);
    }

    /// Every rejection names its stable code and metric reason.
    #[test]
    fn rejection_codes_and_reasons_are_total() {
        assert_eq!(AdmissionReject::GlobalFull.code(), "resource_exhausted");
        assert_eq!(
            AdmissionReject::ActionFull {
                action: "a".to_string()
            }
            .code(),
            "resource_exhausted"
        );
        assert_eq!(AdmissionReject::ParseFull.code(), "resource_exhausted");
        assert_eq!(AdmissionReject::GlobalFull.reason(), "global_inflight");
        assert_eq!(
            AdmissionReject::ActionFull {
                action: "a".to_string()
            }
            .reason(),
            "action_inflight"
        );
        assert_eq!(AdmissionReject::ParseFull.reason(), "parse_pressure");

        assert_eq!(SubscriptionReject::GlobalFull.reason(), "global_full");
        assert_eq!(SubscriptionReject::SubjectFull.reason(), "subject_full");
        assert_eq!(SubscriptionReject::IpFull.reason(), "ip_full");
        assert_eq!(SubscriptionReject::ClientBusy.reason(), "client_busy");
        assert_eq!(SubscriptionReject::RateLimited.reason(), "rate_limited");
    }

    /// `incr_by` accumulates on the existing key and starts a new one with
    /// the delta.
    #[test]
    fn metrics_accumulate_by_delta() {
        let metrics = Metrics::default();
        metrics.incr("requests", &[("result", "ok")]);
        metrics.incr_by("requests", &[("result", "ok")], 4);
        metrics.incr_by("bytes", &[("stage", "raw")], 10);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot["requests{result=ok}"], 5);
        assert_eq!(snapshot["bytes{stage=raw}"], 10);
    }

    /// A lease's Debug names its key without exposing queue internals, and
    /// a lease with an IP releases the per-IP bucket as well.
    #[test]
    fn lease_debug_and_ip_release() {
        let table: toml::Table =
            toml::from_str("[subscriptions]\nper_ip = 1\ncreate_rate_per_sec = 100\n").unwrap();
        let config = Config {
            data: Default::default(),
            table,
        };
        let registry = Ingress::new(&config).subscriptions.clone();
        let ip: IpAddr = "10.9.9.9".parse().unwrap();
        let key = SubscriptionKey {
            transport: TRANSPORT_WEB,
            subject: "s".to_string(),
            client_id: "c".to_string(),
            ip: Some(ip),
        };
        let lease = registry.try_acquire(key.clone()).expect("admitted");
        let debug = format!("{lease:?}");
        assert!(debug.contains("SubscriptionLease"), "{debug}");
        assert_eq!(
            registry
                .try_acquire(SubscriptionKey {
                    transport: TRANSPORT_GRPC,
                    client_id: "d".to_string(),
                    ..key
                })
                .err(),
            Some(SubscriptionReject::IpFull)
        );
        drop(lease);
        assert_eq!(
            registry
                .try_acquire(SubscriptionKey {
                    transport: TRANSPORT_GRPC,
                    subject: "s".to_string(),
                    client_id: "e".to_string(),
                    ip: Some(ip),
                })
                .err(),
            None,
            "the dropped lease released the IP bucket"
        );
    }

    /// The creation window resets after a second.
    #[test]
    fn the_creation_window_resets() {
        let table: toml::Table =
            toml::from_str("[subscriptions]\ncreate_rate_per_sec = 1\nper_subject = 10\n").unwrap();
        let config = Config {
            data: Default::default(),
            table,
        };
        let registry = Ingress::new(&config).subscriptions.clone();
        let key = |client: &str| SubscriptionKey {
            transport: TRANSPORT_WEB,
            subject: "w".to_string(),
            client_id: client.to_string(),
            ip: None,
        };
        let first = registry.try_acquire(key("a")).expect("first admitted");
        assert_eq!(
            registry.try_acquire(key("b")).err(),
            Some(SubscriptionReject::RateLimited)
        );
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let second = registry
            .try_acquire(key("c"))
            .expect("the window reset admits again");
        drop(first);
        drop(second);
    }

    /// An unknown profile class name warns and changes no budget (the row
    /// applies only to an action literally named that).
    #[test]
    fn unknown_profile_class_only_warns() {
        let table: toml::Table =
            toml::from_str("[ingress]\n[ingress.profile.not_a_class]\nmax_inflight = 3\n").unwrap();
        let ingress = ingress_with(table);
        assert_eq!(
            ingress.profile("msg:ack").max_inflight,
            SMALL_PROFILE.max_inflight,
            "an unknown class name leaves the real classes alone"
        );
    }
}
