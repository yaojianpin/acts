//! The web transport's ingress middleware stack (design §4.1).
//!
//! Every request that reaches a route passes, from the handler outwards:
//! correlation id → quiesce gate → global concurrency → per-IP concurrency →
//! per-IP rate limit → request timeout → metrics. A gate refusal is answered
//! through the shared [`AppError`] contract (stable code, correlation id,
//! `x-correlation-id` header) and counted in the engine's ingress metrics —
//! the caller sees the same face for a full transport as for a full engine.
//!
//! The concurrency gate's inflight counter is the same [`InflightCounter`] the
//! [`WebTransport`] handle reports to `Engine::close`, so a quiesce stops
//! admissions at the gate and a drain waits out exactly the requests the gate
//! admitted.

use crate::config::HttpConfig;
use crate::objects::AppError;
use acts::{
    Engine, InflightCounter, Ingress, RunningTransport, TRANSPORT_WEB, TransportDrainReport,
};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

/// The response-header a client reads before retrying a refused request.
const RETRY_AFTER: HeaderName = HeaderName::from_static("retry-after");
/// Longest `X-Forwarded-For` chain parsed: a header longer than this is not a
/// proxy chain, it is payload smuggled into a header.
const MAX_FORWARDED_CHAIN: usize = 16;

/// One parsed CIDR range (`10.0.0.0/8`, `2001:db8::/32`), v4 or v6.
///
/// A small parser of the deployment's own config is preferred over a
/// dependency: the only use is matching the direct peer of a connection
/// against `[web].trusted_proxy_cidrs`, and both the syntax and the semantics
/// (prefix in bits, all-zero host bits accepted) fit in a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parse `addr/prefix` (a bare address is a /32 or /128).
    pub fn parse(text: &str) -> Option<Cidr> {
        let (addr_text, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, prefix.parse::<u8>().ok()?),
            None => (
                text,
                match text.parse::<IpAddr>() {
                    Ok(IpAddr::V4(_)) => 32,
                    Ok(IpAddr::V6(_)) => 128,
                    Err(_) => return None,
                },
            ),
        };
        let addr: IpAddr = addr_text.parse().ok()?;
        let max = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix > max {
            return None;
        }
        Some(Cidr { addr, prefix })
    }

    /// Whether `ip` falls inside the range.
    pub fn contains(&self, ip: &IpAddr) -> bool {
        // A v4 range never contains a v6 address (and vice versa): matching
        // the wrong family would silently trust a peer.
        match (self.addr, *ip) {
            (IpAddr::V4(a), IpAddr::V4(b)) => prefix_match(
                u128::from(u32::from(a)),
                u128::from(u32::from(b)),
                self.prefix,
                32,
            ),
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                prefix_match(u128::from(a), u128::from(b), self.prefix, 128)
            }
            _ => false,
        }
    }
}

/// Whether two addresses share their leading `prefix` bits, compared in a
/// `bits`-wide space (32 for v4, 128 for v6 — a v4 address promoted to u128
/// keeps its bits in the low lane, so the comparison width must follow the
/// family or every short prefix would compare zeros).
fn prefix_match(a: u128, b: u128, prefix: u8, bits: u32) -> bool {
    if prefix == 0 {
        return true;
    }
    let shift = bits - prefix as u32;
    (a >> shift) == (b >> shift)
}

/// The client IP a request is attributed to.
///
/// `X-Forwarded-For` is honored only when the direct peer is itself inside a
/// trusted-proxy range: anyone else setting the header would otherwise choose
/// which bucket their requests are counted against. Of the forwarded chain,
/// the rightmost entry that is not itself a trusted proxy is the client —
/// every entry further right was appended by a proxy this deployment trusts.
pub fn client_ip(peer: IpAddr, forwarded_for: Option<&str>, trusted: &[Cidr]) -> IpAddr {
    let Some(chain) = forwarded_for else {
        return peer;
    };
    if !trusted.iter().any(|range| range.contains(&peer)) {
        return peer;
    }
    chain
        .split(',')
        .rev()
        .take(MAX_FORWARDED_CHAIN)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .find_map(|entry| {
            entry
                .parse::<IpAddr>()
                .ok()
                .filter(|ip| !trusted.iter().any(|range| range.contains(ip)))
        })
        // A chain of nothing but trusted proxies has no client of its own;
        // the peer (the innermost proxy) is the best answer there is.
        .unwrap_or(peer)
}

/// The action a request path resolves to — the same name the route's handler
/// dispatches, so the per-route body cap, the deadline and the metric labels
/// all come from the one profile table. `None` for paths that are not part of
/// the action surface (`/health`).
pub fn route_action(path: &str) -> Option<String> {
    // The api router is nested, so a middleware may see the path with or
    // without the `/api` prefix depending on where it sits in the stack.
    let path = path.strip_prefix("/api").unwrap_or(path);
    let action = match path {
        "/model/list" => "model:ls",
        "/model/get" => "model:get",
        "/model/rm" => "model:rm",
        "/model/deploy" => "model:deploy",
        "/proc/start" => "proc:start",
        "/pack/list" => "pack:ls",
        "/pack/catalogs" => "pack:ls",
        "/pack" => "pack:get",
        "/msg/sse" => "msg:sub",
        "/msg/ack" => "msg:ack",
        "/snap/upsert" => "snap:upsert",
        "/snap/remove" => "snap:remove",
        "/snap/get" => "snap:get",
        "/snap/ls" => "snap:ls",
        "/snap/reply" => "snap:reply",
        other => {
            if let Some(action) = other.strip_prefix("/act/") {
                return Some(format!("act:{action}"));
            }
            if other.starts_with("/hooks/") {
                return Some("evt:start".to_string());
            }
            return None;
        }
    };
    Some(action.to_string())
}

/// One metric label explaining why a request was refused, attached to the
/// refusal response so the outermost metrics middleware can count it without
/// re-deriving the reason from a status code.
#[derive(Debug, Clone, Copy)]
pub struct RejectReason(pub &'static str);

/// The correlation id a request entered with, for handlers that report it.
#[derive(Debug, Clone)]
pub struct CorrelationId(pub Arc<str>);

/// The shared state of the web transport's admission stack: its limits, its
/// rate buckets, and the inflight counter the lifecycle handle watches.
pub struct LimitState {
    /// Inflight counter shared with [`WebTransport`]: the quiesce gate admits
    /// through it, so a quiesce stops admissions and a drain waits out
    /// exactly what was admitted.
    pub counter: Arc<InflightCounter>,
    /// The engine's shared ingress budget — profile lookups and metrics.
    ingress: Arc<Ingress>,
    /// Whole-transport concurrency.
    global: Arc<Semaphore>,
    /// Concurrent requests per client IP (an entry per IP with requests
    /// inflight).
    per_ip: std::sync::Mutex<HashMap<IpAddr, usize>>,
    /// Token buckets per client IP.
    rate: std::sync::Mutex<HashMap<IpAddr, RateBucket>>,
    /// Cap from `[web].max_concurrent_per_ip`.
    max_per_ip: usize,
    /// Budget from `[web].rate_limit_per_min` (`0` = limiter off).
    rate_per_min: u64,
    /// Ranges allowed to set `X-Forwarded-For`.
    trusted: Vec<Cidr>,
}

impl LimitState {
    /// Build from the engine's `[web]` section. Fails the start when a
    /// trusted-proxy range does not parse: a silently dropped entry would
    /// silently un-trust a proxy, and the per-IP limits would then converge
    /// onto the proxies themselves.
    pub fn new(engine: &Engine, config: &HttpConfig) -> acts::Result<Arc<Self>> {
        let trusted = config
            .trusted_proxy_cidrs
            .iter()
            .map(|text| {
                Cidr::parse(text).ok_or_else(|| {
                    acts::ActError::Config(format!(
                        "invalid web.trusted_proxy_cidrs entry '{text}': expected addr/prefix"
                    ))
                })
            })
            .collect::<acts::Result<Vec<_>>>()?;
        Ok(Arc::new(Self {
            counter: Arc::new(InflightCounter::new()),
            ingress: engine.ingress(),
            global: Arc::new(Semaphore::new(config.max_concurrent())),
            per_ip: std::sync::Mutex::new(HashMap::new()),
            rate: std::sync::Mutex::new(HashMap::new()),
            max_per_ip: config.max_concurrent_per_ip(),
            rate_per_min: config.rate_limit_per_min(),
            trusted,
        }))
    }

    /// Resolve the client IP of a request from its peer and forwarded header.
    fn client_ip_of(&self, peer: &SocketAddr, request: &Request) -> IpAddr {
        let forwarded = request
            .headers()
            .get(axum::http::HeaderName::from_static("x-forwarded-for"))
            .and_then(|value| value.to_str().ok());
        client_ip(peer.ip(), forwarded, &self.trusted)
    }

    /// Take one of the client IP's concurrent slots, or refuse: one address
    /// that never finishes its requests must not hold the transport's whole
    /// budget.
    fn try_enter_per_ip(self: &Arc<Self>, ip: IpAddr) -> Option<IpSlot> {
        let mut slots = self.per_ip.lock().expect("per-ip lock");
        let count = slots.entry(ip).or_insert(0);
        if *count >= self.max_per_ip {
            return None;
        }
        *count += 1;
        Some(IpSlot {
            state: self.clone(),
            ip,
        })
    }

    /// Spend one token of the client IP's per-minute budget (`true` = spend),
    /// or refuse. A bucket starts full: the first burst is allowed, and a
    /// steady rate refills at exactly `rate_per_min / 60` tokens per second.
    fn try_spend_rate(&self, ip: IpAddr) -> bool {
        if self.rate_per_min == 0 {
            return true;
        }
        let mut buckets = self.rate.lock().expect("rate lock");
        let now = Instant::now();
        let capacity = self.rate_per_min as f64;
        let refill_per_sec = capacity / 60.0;
        let bucket = buckets.entry(ip).or_insert(RateBucket {
            tokens: capacity,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_sec).min(capacity);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One client IP's token bucket.
struct RateBucket {
    tokens: f64,
    updated: Instant,
}

/// One client IP's concurrent slot. Held across the request; dropping it
/// gives the slot back (and removes the map entry at zero, so a long-gone IP
/// leaves no residue).
struct IpSlot {
    state: Arc<LimitState>,
    ip: IpAddr,
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut slots = self.state.per_ip.lock().expect("per-ip lock");
        if let std::collections::hash_map::Entry::Occupied(mut entry) = slots.entry(self.ip) {
            *entry.get_mut() -= 1;
            if *entry.get() == 0 {
                entry.remove();
            }
        }
    }
}

/// Build a refusal through the shared error contract, tagged with its metric
/// reason (and a `Retry-After` for the exhausted-budget codes).
fn reject(
    code: acts::ExternalCode,
    message: String,
    reason: &'static str,
    retry_after: bool,
) -> Response {
    let mut response = AppError::from_external(acts::ExternalError::new(
        code,
        message,
        acts::new_correlation_id(),
    ))
    .into_response();
    response.extensions_mut().insert(RejectReason(reason));
    if retry_after {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

/// The outermost middleware: count the request once its result is known.
///
/// `result` is `rejected` when a gate refused the request (the refusal
/// carries its [`RejectReason`]), `error` when a handler or the engine
/// failed, `ok` otherwise.
pub async fn metrics(
    State(state): State<Arc<LimitState>>,
    request: Request,
    next: Next,
) -> Response {
    let action = route_action(request.uri().path()).unwrap_or_else(|| "unknown".to_string());
    let response = next.run(request).await;
    let (result, reason) = match response.extensions().get::<RejectReason>() {
        Some(reason) => ("rejected", Some(reason.0)),
        None if response.status().is_client_error() || response.status().is_server_error() => {
            ("error", None)
        }
        None => ("ok", None),
    };
    state.ingress.metrics.incr(
        "acts_ingress_requests_total",
        &[
            ("transport", TRANSPORT_WEB),
            ("action", &action),
            ("result", result),
        ],
    );
    if let Some(reason) = reason {
        state.ingress.metrics.incr(
            "acts_ingress_rejected_total",
            &[("transport", TRANSPORT_WEB), ("reason", reason)],
        );
    }
    response
}

/// The innermost middleware: mint (or keep) a correlation id, open the
/// request's `ingress{transport,action,cid}` span, run the request inside
/// the correlation scope (so every error body answers the same id this
/// header carries), and stamp every response with `x-correlation-id`.
pub async fn correlation(request: Request, next: Next) -> Response {
    let cid = acts::new_correlation_id();
    let action = route_action(request.uri().path()).unwrap_or_else(|| "unknown".to_string());
    let span = tracing::info_span!(
        "ingress",
        transport = TRANSPORT_WEB,
        action = %action,
        cid = %cid,
    );
    let mut request = request;
    request
        .extensions_mut()
        .insert(CorrelationId(cid.clone().into()));
    let mut response = {
        let _entered = span.enter();
        // The task-local scope is what makes `AppError`'s constructors (and
        // every other error face built inside the handler) reuse this id
        // instead of minting a second one.
        acts::export::error::scoped(cid.clone(), next.run(request)).await
    };
    if let Ok(value) = HeaderValue::from_str(&cid) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-correlation-id"), value);
    }
    response
}

/// Refuse everything once the transport is quiescing; admit (and hold a
/// slot for) everything else until the drain finishes.
pub async fn quiesce_gate(
    State(state): State<Arc<LimitState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(_guard) = state.counter.enter() else {
        return reject(
            acts::ExternalCode::Unavailable,
            "the service is shutting down and no longer accepts requests".to_string(),
            "quiescing",
            false,
        );
    };
    next.run(request).await
}

/// Refuse when the whole transport is at `[web].max_concurrent`.
pub async fn global_concurrency(
    State(state): State<Arc<LimitState>>,
    request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = state.global.clone().try_acquire_owned() else {
        return reject(
            acts::ExternalCode::ResourceExhausted,
            "the service is at its request capacity, retry shortly".to_string(),
            "global_inflight",
            true,
        );
    };
    next.run(request).await
}

/// Refuse when the client IP already holds `[web].max_concurrent_per_ip`
/// requests. The IP is the forwarded client when the direct peer is a trusted
/// proxy, the peer's own address otherwise.
pub async fn per_ip_concurrency(
    State(state): State<Arc<LimitState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let ip = state.client_ip_of(&peer, &request);
    let Some(_slot) = state.try_enter_per_ip(ip) else {
        return reject(
            acts::ExternalCode::ResourceExhausted,
            "this client is at its concurrent-request limit, retry shortly".to_string(),
            "per_ip_inflight",
            true,
        );
    };
    next.run(request).await
}

/// Token-bucket rate limit per client IP (`[web].rate_limit_per_min`, `0`
/// turns it off).
pub async fn rate_limit(
    State(state): State<Arc<LimitState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let ip = state.client_ip_of(&peer, &request);
    if !state.try_spend_rate(ip) {
        return reject(
            acts::ExternalCode::ResourceExhausted,
            "this client is over its request rate, retry shortly".to_string(),
            "rate_limited",
            true,
        );
    }
    next.run(request).await
}

/// Cancel the request past its action's profile deadline: the caller gets a
/// `503 deadline_exceeded` and the admission permits (counted inflight) are
/// released as this future is dropped.
pub async fn timeout(
    State(state): State<Arc<LimitState>>,
    request: Request,
    next: Next,
) -> Response {
    let deadline_ms = route_action(request.uri().path())
        .map(|action| state.ingress.profile(&action).deadline_ms)
        .unwrap_or(crate::objects::FALLBACK_DEADLINE_MS);
    match tokio::time::timeout(Duration::from_millis(deadline_ms), next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            let mut response = AppError::deadline_exceeded(format!(
                "the request exceeded its {deadline_ms} ms deadline"
            ))
            .into_response();
            response.extensions_mut().insert(RejectReason("deadline"));
            response
        }
    }
}

/// Wrap `router` with the middleware stack, innermost first: correlation
/// (closest to the handlers) up to metrics (outermost). Applied as
/// `route_layer`s so routes registered after (the open `/health`) stay out of
/// both the stack and the metrics.
pub fn apply_layers<S>(state: &Arc<LimitState>, router: axum::Router<S>) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .route_layer(axum::middleware::from_fn(correlation))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            quiesce_gate,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            global_concurrency,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            per_ip_concurrency,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit,
        ))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), timeout))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), metrics))
}

/// The lifecycle face of the web transport: `Engine::close` quiesces and
/// drains this before plugin shutdown. It watches exactly the counter the
/// quiesce gate admits through, so "drained" means every request the gate
/// admitted has finished.
pub struct WebTransport {
    counter: Arc<InflightCounter>,
}

impl WebTransport {
    pub fn new(counter: Arc<InflightCounter>) -> Self {
        Self { counter }
    }
}

#[async_trait::async_trait]
impl RunningTransport for WebTransport {
    fn transport(&self) -> &'static str {
        TRANSPORT_WEB
    }

    async fn quiesce(&self) {
        self.counter.quiesce();
    }

    async fn drain(&self, deadline: Duration) -> TransportDrainReport {
        self.counter.drain(TRANSPORT_WEB, deadline).await
    }
}

#[cfg(test)]
mod tests;
