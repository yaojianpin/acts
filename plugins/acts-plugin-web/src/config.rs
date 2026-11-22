use serde::Deserialize;

/// Host the HTTP transport binds when `[web].host` is not set: loopback only,
/// so the management surface is not reachable from the network by default.
/// Set `host = "0.0.0.0"` to serve remote callers.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Messages that may wait for one SSE subscriber when `[web].queue_size` is
/// not set.
pub const DEFAULT_QUEUE_SIZE: usize = 100;

/// Upper bound of `[web].queue_size`: a subscriber this far behind is not
/// going to catch up by being given more room.
const MAX_QUEUE_SIZE: usize = 65_536;

/// Concurrent requests the whole transport admits when `[web].max_concurrent`
/// is not set.
pub const DEFAULT_MAX_CONCURRENT: usize = 512;
/// Concurrent requests one client IP may hold when
/// `[web].max_concurrent_per_ip` is not set.
pub const DEFAULT_MAX_CONCURRENT_PER_IP: usize = 64;
/// Requests per client IP per minute when `[web].rate_limit_per_min` is not
/// set.
pub const DEFAULT_RATE_LIMIT_PER_MIN: u64 = 600;

/// Sane ceiling for the whole-transport concurrency: past it the limit is no
/// longer a limit, it is just the engine's own admission budget (`[ingress]`)
/// deciding.
const MAX_MAX_CONCURRENT: usize = 65_536;
/// Sane ceiling for the per-IP concurrency: one client IP that honestly needs
/// more than this holds connections it never finishes.
const MAX_MAX_CONCURRENT_PER_IP: usize = 16_384;
/// Sane ceiling for the per-IP rate: past it the token bucket refills faster
/// than a client can spend.
const MAX_RATE_LIMIT_PER_MIN: u64 = 1_000_000;

/// `[web]` — the HTTP transport, including the SSE message stream.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct HttpConfig {
    /// Address the web server binds (`[web].host`). Defaults to
    /// [`DEFAULT_HOST`] — loopback only — so a deployment that wants the
    /// transport reachable from other hosts says so explicitly.
    pub host: Option<String>,

    pub port: Option<u32>,

    /// How many messages may wait for one SSE subscriber.
    ///
    /// The queue is the transport's only backlog for a subscription: a message
    /// that does not fit is never awaited on, and the subscriber whose queue is
    /// full is disconnected instead (see `sse::sse`). An unacked delivery of a
    /// process that has not settled stays in the store, so the engine's retry
    /// timer re-sends it to the channel when the client subscribes again.
    pub queue_size: Option<usize>,

    /// Concurrent requests the whole transport admits. A refusal is answered
    /// `429` with a `Retry-After` header — the transport, not the engine, is
    /// the one at capacity here. `0` (or an absurd value) selects the default.
    pub max_concurrent: Option<usize>,

    /// Concurrent requests one client IP may hold at once: one address that
    /// opens sockets without ever finishing them must not hold the whole
    /// transport's budget. `0` selects the default.
    pub max_concurrent_per_ip: Option<usize>,

    /// Requests one client IP may make per minute, enforced with a token
    /// bucket. `0` turns the limiter off (a trusted internal deployment).
    pub rate_limit_per_min: Option<u64>,

    /// CIDR ranges (`10.0.0.0/8`, `fd00::/8`) of reverse proxies allowed to
    /// set `X-Forwarded-For`. A peer inside these ranges has its forwarded
    /// client IP (the last untrusted entry of the chain) used for the per-IP
    /// limits; any other peer's header is ignored and its own address is the
    /// client.
    #[serde(default)]
    pub trusted_proxy_cidrs: Vec<String>,

    /// Accept plain HTTP on a non-loopback bind — only for deployments that
    /// front the transport with their own TLS-terminating proxy on a protected
    /// network. With `false` (the default) strict mode refuses to start a
    /// non-loopback bind that has no `[web].tls`.
    #[serde(default)]
    pub plain_behind_proxy: bool,

    /// TLS for the listener. `cert`/`key` are PEM file paths; `client_ca`,
    /// when set, turns on mutual TLS (clients must present a certificate
    /// signed by that CA).
    pub tls: Option<TlsConfig>,

    /// The posture level of this section (`permissive` | `standard` |
    /// `strict`). Omitted, it follows `[security].level` (which follows
    /// `standard`). The level drives the strict-plaintext rule: `permissive`
    /// only warns on a plain non-loopback bind, `standard` refuses it unless
    /// `tls` or `plain_behind_proxy` is set, `strict` refuses unless `tls`
    /// is set. The concurrency and rate defaults below do NOT scale with the
    /// level — they are transport sizing, not posture.
    pub level: Option<String>,
}

/// `[web].tls` — the listener's certificate chain, private key and optional
/// client CA, all PEM file paths read at start.
#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    /// PEM file with the server certificate (and its chain).
    pub cert: String,
    /// PEM file with the private key matching `cert`.
    pub key: String,
    /// PEM file with the CA that must have signed every client certificate
    /// (mutual TLS) — omitted, client certificates are not requested.
    pub client_ca: Option<String>,
}

impl HttpConfig {
    /// The per-subscriber queue capacity, clamped so that neither `0` (a
    /// subscription that can never receive a message) nor an absurd value (the
    /// unbounded backlog the queue exists to prevent) is configurable.
    pub fn queue_size(&self) -> usize {
        self.queue_size
            .unwrap_or(DEFAULT_QUEUE_SIZE)
            .clamp(1, MAX_QUEUE_SIZE)
    }

    /// The whole-transport concurrency cap, clamped like the queue: `0` means
    /// "not configured" (a transport that admits nothing would refuse every
    /// request) and the ceiling keeps the value a real bound.
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
            .unwrap_or(DEFAULT_MAX_CONCURRENT)
            .clamp(1, MAX_MAX_CONCURRENT)
    }

    /// The per-IP concurrency cap, clamped the same way.
    pub fn max_concurrent_per_ip(&self) -> usize {
        self.max_concurrent_per_ip
            .unwrap_or(DEFAULT_MAX_CONCURRENT_PER_IP)
            .clamp(1, MAX_MAX_CONCURRENT_PER_IP)
    }

    /// The per-IP rate, in requests per minute. Unlike the concurrency caps,
    /// `0` is meaningful: the limiter is off.
    pub fn rate_limit_per_min(&self) -> u64 {
        match self.rate_limit_per_min {
            Some(0) => 0,
            Some(v) => v.clamp(1, MAX_RATE_LIMIT_PER_MIN),
            None => DEFAULT_RATE_LIMIT_PER_MIN,
        }
    }
}
