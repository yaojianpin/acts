use serde::Deserialize;

/// Host the gRPC server binds when `[grpc].host` is not set: loopback only,
/// so the service is not reachable from the network by default. Set
/// `host = "0.0.0.0"` to serve remote callers.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Messages that may wait for one `on_message` subscriber when
/// `[grpc].queue_size` is not set.
pub const DEFAULT_QUEUE_SIZE: usize = 128;

/// Upper bound of `[grpc].queue_size`: a subscriber this far behind is not
/// going to catch up by being given more room.
const MAX_QUEUE_SIZE: usize = 65_536;

/// Server HTTP/2 ping interval when `[grpc].http2_keepalive_secs` is not
/// set. A ping every 30 s is well under typical idle-timeout firewalls.
pub const DEFAULT_HTTP2_KEEPALIVE_SECS: u64 = 30;

/// Server HTTP/2 ping timeout when `[grpc].http2_keepalive_timeout_secs` is
/// not set.
pub const DEFAULT_HTTP2_KEEPALIVE_TIMEOUT_SECS: u64 = 15;

/// `[grpc].tls` — server certificate, private key, and optionally the CA
/// that must have issued the client certificate. Paths to PEM files.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TlsConfig {
    /// PEM file with the server certificate chain.
    pub cert: String,
    /// PEM file with the server private key.
    pub key: String,
    /// PEM file with the CA the client certificate must chain to. Setting
    /// it turns on mutual TLS: a connection without a client certificate is
    /// refused at the TLS handshake.
    pub client_ca: Option<String>,
}

/// `[grpc]` — the gRPC server.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct GrpcConfig {
    /// Address the gRPC server binds (`[grpc].host`). Defaults to
    /// [`DEFAULT_HOST`] — loopback only — so a deployment that wants the
    /// service reachable from other hosts says so explicitly.
    pub host: Option<String>,

    pub port: Option<u32>,
    /// How many messages may wait for one `on_message` subscriber.
    ///
    /// The queue is the transport's only backlog for an RPC: a message that
    /// does not fit is never awaited on, and the subscriber whose queue is full
    /// has its stream ended instead (see `MessageClient::send`). An unacked
    /// delivery of a process that has not settled stays in the store, so the
    /// engine's retry timer re-sends it to the channel when the client
    /// subscribes again.
    pub queue_size: Option<usize>,

    /// Seconds between server HTTP/2 pings (`[grpc].http2_keepalive_secs`).
    /// A ping every 30 s: a half-dead connection (peer gone, TCP still
    /// established) is killed by the ping timeout instead of holding a
    /// stream slot forever.
    pub http2_keepalive_secs: Option<u64>,

    /// Seconds the server waits for a ping answer
    /// (`[grpc].http2_keepalive_timeout_secs`) before closing the
    /// connection.
    pub http2_keepalive_timeout_secs: Option<u64>,

    /// Server TLS (`[grpc].tls`). Absent: serve plaintext.
    pub tls: Option<TlsConfig>,

    /// Acknowledge that the plaintext service sits behind a TLS-terminating
    /// proxy (`[grpc].plain_behind_proxy`). Under the `standard` level this
    /// is the only way a non-loopback bind is accepted without
    /// `[grpc].tls`; under `strict` it is not enough on its own.
    pub plain_behind_proxy: bool,

    /// The posture level of this section (`[grpc].level`). Omitted, the
    /// `[security].level` decides; the level presets the strict-mode rule
    /// for a non-loopback plaintext bind (permissive warns, standard
    /// refuses without TLS or this flag, strict refuses without TLS).
    pub level: Option<String>,
}

impl GrpcConfig {
    /// The per-subscriber queue capacity, clamped so that neither `0` (a
    /// subscription that can never receive a message) nor an absurd value (the
    /// unbounded backlog the queue exists to prevent) is configurable.
    pub fn queue_size(&self) -> usize {
        self.queue_size
            .unwrap_or(DEFAULT_QUEUE_SIZE)
            .clamp(1, MAX_QUEUE_SIZE)
    }

    /// Seconds between server HTTP/2 pings, clamped: a ping more often than
    /// every 5 s is a busy-loop on idle connections, one less often than
    /// every 10 min lets dead connections outlive it.
    pub fn http2_keepalive_secs(&self) -> u64 {
        self.http2_keepalive_secs
            .unwrap_or(DEFAULT_HTTP2_KEEPALIVE_SECS)
            .clamp(5, 600)
    }

    /// Seconds to wait for a ping answer, clamped: shorter than 1 s turns
    /// any scheduling hiccup into a disconnect, longer than 10 min keeps a
    /// half-dead connection around.
    pub fn http2_keepalive_timeout_secs(&self) -> u64 {
        self.http2_keepalive_timeout_secs
            .unwrap_or(DEFAULT_HTTP2_KEEPALIVE_TIMEOUT_SECS)
            .clamp(1, 600)
    }
}
