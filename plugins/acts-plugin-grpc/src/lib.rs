// `tonic::Status` (~176 bytes) is the gRPC error type; boxing it would break the
// service trait, so suppress the large-Result lint.
#![allow(clippy::result_large_err)]

use acts::{
    ActPlugin, Channel, ChannelOptions, Engine, InflightCounter, SubscriptionKey, TRANSPORT_GRPC,
    Vars,
};
use acts_proto::{Message, MessageOptions, acts_service_server::*};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::AsyncWrite as _IoWrite;
use tokio::io::{AsyncRead as _IoRead, ReadBuf as _IoReadBuf};
use tokio::sync::mpsc::{self, Sender, error::TrySendError};
use tokio_stream::{Stream, StreamExt as _, wrappers::ReceiverStream};
use tonic::{Code, Response, Status, transport::Server};

pub use config::{DEFAULT_HOST, DEFAULT_QUEUE_SIZE, GrpcConfig, TlsConfig};

mod config;

/// Per-connection IO handed to tonic: with TLS configured it is a
/// `TlsStream<TcpStream>`, without it the raw `TcpStream`. A trait object
/// keeps the server builder the same in both cases; the wrapper carries the
/// peer address because tonic reads it through `Connected` (the peer IP is
/// what the subscription registry's per-IP quota keys on).
/// The object-safe supertrait the boxed connection hides behind: a single
/// non-auto trait is what a trait object may carry.
trait GrpcIo: _IoRead + _IoWrite + Send + Unpin {}
impl<T: _IoRead + _IoWrite + Send + Unpin> GrpcIo for T {}

struct GrpcConn {
    io: Box<dyn GrpcIo>,
    peer: Option<std::net::SocketAddr>,
}

impl _IoRead for GrpcConn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut _IoReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl _IoWrite for GrpcConn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

impl tonic::transport::server::Connected for GrpcConn {
    type ConnectInfo = tonic::transport::server::TcpConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        tonic::transport::server::TcpConnectInfo {
            local_addr: None,
            remote_addr: self.peer,
        }
    }
}

type Conn = GrpcConn;
type Incoming = Pin<Box<dyn Stream<Item = std::io::Result<Conn>> + Send>>;

type MessageStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<Message, Status>> + Send>>;

/// Wire-field bounds enforced before any allocation from the request. These
/// are transport-level sanity caps, not the action's own payload budget (the
/// profile's `max_raw_bytes` covers that): no gRPC field may be big enough
/// to make a parse allocation-heavy before the permit is even held.
const MAX_FIELD_BYTES: usize = 256;
const MAX_ACK_BYTES: usize = 1024;
const MAX_OPTION_ENTRIES: usize = 64;
const MAX_OPTION_KEY_BYTES: usize = 128;
const MAX_OPTION_VALUE_BYTES: usize = 1024;

/// Explicit message caps on the generated server, replacing tonic's
/// framework defaults: an 8 MiB request is already far past any action's
/// payload budget, and the 16 MiB reply cap keeps a snapshot echo bounded.
const MAX_DECODING_MESSAGE_SIZE: usize = 8 * 1024 * 1024;
const MAX_ENCODING_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// The start-time failure of registering the pre-bound listener with the
/// runtime. Built through a named function so the deployment fault it reports
/// (a listener tokio refuses) has one description whether it is raised by the
/// plugin or asserted by a test.
fn register_listener_error(addr: std::net::SocketAddr, err: std::io::Error) -> acts::ActError {
    acts::ActError::Config(format!(
        "failed to register the gRPC listener on {addr}: {err}"
    ))
}

/// The start-time failure of putting the bound listener in non-blocking mode.
/// Named for the same reason as [`register_listener_error`]: the fault is
/// asserted directly, not only through a bind that happens to fail this way.
fn configure_listener_error(addr: std::net::SocketAddr, err: std::io::Error) -> acts::ActError {
    acts::ActError::Config(format!(
        "failed to configure the gRPC listener on {addr}: {err}"
    ))
}

/// Put an [`acts::export::ExternalError`] onto the wire as a gRPC status:
/// the stable code mapped onto the gRPC code, the safe message as the status
/// message, and the full external-error shape (code, correlation id,
/// retryable) in the status details JSON — the same fields the web transport
/// answers with.
fn external_status(external: acts::export::ExternalError) -> Status {
    let code = match external.code.as_str() {
        "not_found" => Code::NotFound,
        "invalid_argument" => Code::InvalidArgument,
        "unauthenticated" => Code::Unauthenticated,
        "permission_denied" => Code::PermissionDenied,
        "conflict" => Code::Aborted,
        "resource_exhausted" => Code::ResourceExhausted,
        "busy" => Code::ResourceExhausted,
        "unavailable" => Code::Unavailable,
        _ => Code::Internal,
    };
    let details = serde_json::to_vec(&external.to_json())
        .map(bytes::Bytes::from)
        .unwrap_or_default();
    Status::with_details(code, external.message, details)
}

/// The refusal for one ingress rejection: the external-error shape with the
/// stable code (`resource_exhausted`, `busy`) and its own correlation id,
/// the same refusal body every transport answers with.
fn reject_status(code: acts::export::ExternalCode, message: String) -> Status {
    let correlation_id = acts::export::new_correlation_id();
    external_status(acts::export::ExternalError::new(
        code,
        message,
        correlation_id,
    ))
}

/// Map an action-dispatch failure onto its gRPC status, so the unary and the
/// subscription paths answer a refusal the same way — and the same way every
/// other transport does: a stable code in the status details (JSON, the
/// [`acts::export::ExternalError`] shape), the safe message as the status
/// message, and an `internal` failure carrying none of its store-path or SQL
/// detail (that goes to the log under the correlation id).
fn action_status(err: acts::actions::Error) -> Status {
    let correlation_id = acts::export::new_correlation_id();
    let external = acts::export::ExternalError::from_action(&err, &correlation_id);
    external_status(external)
}

/// The `invalid_argument` refusal for a wire field that passed its bound:
/// the field name and its cap, no echo of the oversized value itself.
fn invalid_field(field: &str, max_bytes: usize) -> Status {
    reject_status(
        acts::export::ExternalCode::Invalid,
        format!("{field} exceeds its limit of {max_bytes} bytes"),
    )
}

/// Validate the wire fields of an action request BEFORE any allocation from
/// them: no `String` is cloned and no payload decoded until every field has
/// passed its transport-level cap. The payload's own budget is checked
/// separately against the profile (`max_raw_bytes`) — but its size is read
/// from the message, not its contents.
fn validate_message(message: &Message, max_raw_bytes: usize) -> Result<(), Status> {
    if message.name.len() > MAX_FIELD_BYTES {
        return Err(invalid_field("name", MAX_FIELD_BYTES));
    }
    if message.seq.len() > MAX_FIELD_BYTES {
        return Err(invalid_field("seq", MAX_FIELD_BYTES));
    }
    if let Some(ack) = &message.ack
        && ack.len() > MAX_ACK_BYTES
    {
        return Err(invalid_field("ack", MAX_ACK_BYTES));
    }
    if let Some(data) = &message.data
        && data.len() > max_raw_bytes
    {
        return Err(reject_status(
            acts::export::ExternalCode::ResourceExhausted,
            format!("payload exceeds its limit of {max_raw_bytes} bytes"),
        ));
    }
    Ok(())
}

/// Validate a subscription request's wire fields (the same pre-allocation
/// rule as [`validate_message`]): the client id declares the subscriber, the
/// options map is bounded so a client cannot make the engine allocate an
/// unbounded key/value set per connection.
fn validate_message_options(options: &MessageOptions) -> Result<(), Status> {
    if options.client_id.len() > MAX_FIELD_BYTES {
        return Err(invalid_field("client_id", MAX_FIELD_BYTES));
    }
    if options.options.len() > MAX_OPTION_ENTRIES {
        return Err(invalid_field("options", MAX_OPTION_ENTRIES));
    }
    for (key, value) in &options.options {
        if key.len() > MAX_OPTION_KEY_BYTES {
            return Err(invalid_field("option key", MAX_OPTION_KEY_BYTES));
        }
        if value.len() > MAX_OPTION_VALUE_BYTES {
            return Err(invalid_field("option value", MAX_OPTION_VALUE_BYTES));
        }
    }
    Ok(())
}

/// Deregisters the channel handler when the gRPC response stream is dropped
/// (client disconnected or the RPC ended). Without this, every finished
/// `on_message` RPC would leak a handler into the engine emitter: the map
/// grows unboundedly and every future message pays glob matching plus
/// ack-delivery store writes for dead clients.
struct CloseChannelOnDrop {
    chan: Arc<Channel>,
}

impl Drop for CloseChannelOnDrop {
    fn drop(&mut self) {
        self.chan.close();
    }
}

/// Response stream wrapper that deregisters the channel handler when the
/// stream is dropped by the transport. It also holds the subscription lease
/// the stream was admitted under (dropped with the stream, so its quota
/// slots free for the next subscriber) and reports its liveness to the
/// transport handle, whose drain report counts live streams.
struct GuardedMessageStream {
    inner: ReceiverStream<Result<Message, Status>>,
    _guard: CloseChannelOnDrop,
    /// The lease's drop releases the registry slots; holding it here makes
    /// the subscription live exactly as long as the response stream.
    _lease: acts::SubscriptionLease,
    streams: Arc<AtomicUsize>,
}

impl Drop for GuardedMessageStream {
    fn drop(&mut self) {
        self.streams.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Stream for GuardedMessageStream {
    type Item = Result<Message, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

/// One `on_message` subscriber: its queue, its channel options and the channel
/// it registered (shared, so a handler invocation clones an `Arc`, not this).
struct MessageClient {
    addr: String,
    sender: Sender<Result<Message, Status>>,
    options: ChannelOptions,
    /// The channel this stream registered, used to end the subscription of a
    /// client that stopped reading. Weak because the handler that holds this
    /// client is owned by the engine's emitter, which the channel's own runtime
    /// owns: a strong reference would keep the runtime alive from inside itself.
    chan: Weak<Channel>,
}

impl MessageClient {
    /// Hand one message (or one stream error) to the client and never wait for
    /// it: the bounded queue is the whole backlog of an RPC, and a message that
    /// does not fit means the client stopped reading — the stream is ended
    /// there and then. Spawning a task per message to await room instead made
    /// the queue no bound at all: every message past it left a task (and the
    /// message it held) alive until the client read, which a client that never
    /// reads never does.
    ///
    /// A client disconnected this way loses nothing the engine still owes the
    /// channel: a delivery that was handed over but not acked, of a process
    /// that has not settled, is re-sent by the retry timer to the channel the
    /// client registers again (the same client id composes the same key).
    fn send(&self, message: Result<Message, Status>) {
        match self.sender.try_send(message) {
            Ok(()) => {
                tracing::info!("send to {}({})", self.addr, self.options.id);
            }
            Err(TrySendError::Full(_)) => {
                tracing::warn!(
                    "client {}({}) is not reading; closing the subscription",
                    self.addr,
                    self.options.id
                );
                if let Some(chan) = self.chan.upgrade() {
                    chan.close();
                }
            }
            // the response stream (and so the receiver) is already gone
            Err(TrySendError::Closed(_)) => {
                tracing::warn!("client {}({}) is closed", self.addr, self.options.id);
            }
        }
    }
}

#[derive(Clone)]
pub struct GrpcServer {
    engine: Engine,
    /// The unary dispatch counter the transport handle drains on: every
    /// admitted `send` holds one guard, so quiesce refuses new work and
    /// drain waits out the dispatches already running.
    inflight: Arc<InflightCounter>,
    /// Live `on_message` streams; counted with the unary counter for the
    /// drain report.
    streams: Arc<AtomicUsize>,
}

impl GrpcServer {
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
            inflight: Arc::new(InflightCounter::new()),
            streams: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The shared form the plugin builds: the plugin keeps the same counter
    /// and stream gauge for its [`GrpcTransport`], whose quiesce/drain must
    /// see exactly the work this server admits.
    fn with_shared(
        engine: &Engine,
        inflight: Arc<InflightCounter>,
        streams: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            engine: engine.clone(),
            inflight,
            streams,
        }
    }

    /// The request's bearer token: the `authorization: Bearer <token>`
    /// metadata entry. Absent metadata is "no credential" — the ACL decides
    /// whether that is acceptable.
    fn bearer_token(request: &tonic::Request<impl Sized>) -> Option<String> {
        request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
    }

    /// Count one request outcome. The action label is the (already
    /// length-capped) request name; the result is `ok`, `error` or
    /// `rejected`, mirroring the web transport's labels.
    fn count_request(&self, action: &str, result: &str) {
        self.engine.ingress().metrics.incr(
            "acts_ingress_requests_total",
            &[
                ("transport", TRANSPORT_GRPC),
                ("action", action),
                ("result", result),
            ],
        );
    }

    /// Count one refusal with its reason — the same metric the web and NATS
    /// transports feed, so one chart shows where requests are turned away.
    fn count_rejected(&self, reason: &str) {
        self.engine.ingress().metrics.incr(
            "acts_ingress_rejected_total",
            &[("transport", TRANSPORT_GRPC), ("reason", reason)],
        );
    }

    async fn do_action(
        &self,
        message: Message,
        token: Option<String>,
        client_timeout: Option<Duration>,
    ) -> Result<Response<Message>, Status> {
        // Quiesce first: once the transport is draining, nothing is admitted
        // and nothing below (auth lookups, permit takes) is even started.
        let Some(_inflight) = self.inflight.enter() else {
            return Err(reject_status(
                acts::export::ExternalCode::Unavailable,
                "the server is shutting down".to_string(),
            ));
        };
        let ingress = self.engine.ingress();
        let profile = ingress.profile(&message.name);

        // Wire-field bounds and the payload budget are checked before
        // anything is cloned or decoded out of the request.
        validate_message(&message, profile.max_raw_bytes)?;

        let name = message.name.clone();
        let principal = match self.engine.acl().resolve(token.as_deref()).await {
            acts::AuthenticationResult::Authenticated(principal) => principal,
            acts::AuthenticationResult::MissingCredentials => self.engine.acl().anonymous(),
            // the actions whose own payload is the credential carry on as the
            // anonymous caller: refusing them would make the very request
            // that repairs a dead session (login, refresh, logout) impossible
            acts::AuthenticationResult::InvalidCredentials
                if acts::carries_own_credential(&name) =>
            {
                self.engine.acl().anonymous()
            }
            acts::AuthenticationResult::InvalidCredentials => {
                self.count_request(&name, "rejected");
                self.count_rejected("unauthenticated");
                return Err(Status::unauthenticated(
                    "the credential is invalid or expired",
                ));
            }
        };

        tracing::info!(
            "do-action seq={} name={} authenticated={}",
            acts::export::safe_text(&message.seq, MAX_FIELD_BYTES),
            name,
            token.is_some()
        );

        // The parse permit brackets the payload decode: a burst of oversized
        // or malformed payloads cannot occupy the engine's parse capacity
        // past the decode itself.
        let Some(_parse) = ingress.try_enter_parse() else {
            self.count_request(&name, "rejected");
            self.count_rejected("parse_pressure");
            return Err(reject_status(
                acts::export::ExternalCode::ResourceExhausted,
                "the engine is at its parse limit; retry later".to_string(),
            ));
        };
        let options = match &message.data {
            // `data` is the action payload and MUST be a JSON object. A
            // malformed payload is a caller error, never an empty option set:
            // silently defaulting to empty options would run the action's
            // global branch (e.g. `msg:clear` clearing every error delivery)
            // instead of rejecting the request.
            Some(data) => serde_json::from_slice::<Vars>(data).map_err(|err| {
                self.count_request(&name, "error");
                Status::invalid_argument(format!("invalid message data: {err}"))
            })?,
            None => Vars::new(),
        };
        drop(_parse);

        // The action permits (global + per-action inflight) are held across
        // the dispatch, so the action's concurrency cap is what the profile
        // says, not what the transport lets through.
        let _permit = match ingress.try_enter_action(&name) {
            Ok(permit) => permit,
            Err(reject) => {
                tracing::warn!(
                    action = %name,
                    reason = reject.reason(),
                    "action admission refused"
                );
                self.count_request(&name, "rejected");
                self.count_rejected(reject.reason());
                return Err(reject_status(
                    acts::export::ExternalCode::ResourceExhausted,
                    format!("the action is at capacity ({})", reject.code()),
                ));
            }
        };

        // The dispatch runs under the tighter of the client's own gRPC
        // timeout and the action profile's server-side deadline. On elapse
        // the client answers `unavailable`, but the engine operation is
        // durable: the client reconciles through reads, it must not retry
        // the write blindly.
        let deadline = Duration::from_millis(profile.deadline_ms);
        let budget = client_timeout.map_or(deadline, |client| client.min(deadline));
        let result = tokio::time::timeout(
            budget,
            acts::actions::apply_as(&self.engine, &principal, &name, options),
        )
        .await;
        let value = match result {
            Err(_) => {
                tracing::warn!(action = %name, budget_ms = budget.as_millis() as u64, "action dispatch timed out");
                self.count_request(&name, "error");
                return Err(reject_status(
                    acts::export::ExternalCode::Unavailable,
                    // the wording matters: the client's next step is a read,
                    // not a retry of a request that may already have applied
                    "the operation did not finish in time; its effect may have applied — reconcile via reads (deadline_exceeded)".to_string(),
                ));
            }
            Ok(Err(err)) => {
                self.count_request(&name, "error");
                return Err(action_status(err));
            }
            Ok(Ok(value)) => value,
        };
        self.count_request(&name, "ok");

        let mut response = Message {
            name,
            seq: acts_proto::create_seq(),
            ack: None,
            data: Some(serde_json::to_vec(&value).map_err(|e| Status::internal(e.to_string()))?),
        };
        if !message.seq.is_empty() {
            response.ack = Some(message.seq.clone());
        }
        Ok(Response::new(response))
    }
}

#[tonic::async_trait]
impl ActsService for GrpcServer {
    type OnMessageStream = MessageStream;
    async fn on_message(
        &self,
        req: tonic::Request<MessageOptions>,
    ) -> Result<tonic::Response<Self::OnMessageStream>, tonic::Status> {
        // A subscription is a read of the message stream, so it goes through
        // the action table like every other request: `msg:sub` must be in the
        // caller's `allow` list, and the key the action answers with is the
        // key this stream registers under — the transport never composes it
        // on its own, so a checked subscription and the key it occupies
        // cannot drift apart.
        let token = Self::bearer_token(&req);
        let principal = match self.engine.acl().resolve(token.as_deref()).await {
            acts::AuthenticationResult::Authenticated(principal) => principal,
            acts::AuthenticationResult::MissingCredentials => self.engine.acl().anonymous(),
            acts::AuthenticationResult::InvalidCredentials => {
                return Err(Status::unauthenticated(
                    "the credential is invalid or expired",
                ));
            }
        };
        let addr = req
            .remote_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let peer_ip = req.remote_addr().map(|addr| addr.ip());
        let options = req.into_inner();
        // Field bounds before anything is cloned out of the request — the
        // same pre-allocation rule the unary path applies.
        validate_message_options(&options)?;
        let client_id = options.client_id.clone();

        let chan_id = acts::actions::apply_as(
            &self.engine,
            &principal,
            acts::ACTION_SUBSCRIBE,
            Vars::new().with("client_id", &client_id),
        )
        .await
        .map_err(action_status)?
        .as_str()
        .unwrap_or_default()
        .to_string();

        // The subscription quota lease comes BEFORE the queue allocation: a
        // refusal must not have allocated the mpsc backlog (or registered a
        // handler) that the refusal then has to unwind. The lease lives as
        // long as the stream (it is moved into the guarded wrapper below).
        let lease = self
            .engine
            .ingress()
            .subscriptions
            .try_acquire(SubscriptionKey {
                transport: TRANSPORT_GRPC,
                // The channel key is `<subject>/<client id>`
                // (`subscription_id`): the subject bucket aggregates every
                // client of one principal, the client bucket dedupes one
                // principal's own connections.
                subject: chan_id
                    .split_once('/')
                    .map(|(subject, _)| subject.to_string())
                    .unwrap_or_else(|| chan_id.clone()),
                client_id: chan_id
                    .split_once('/')
                    .map(|(_, client)| client.to_string())
                    .filter(|client| !client.is_empty())
                    .unwrap_or_else(|| client_id.clone()),
                ip: peer_ip,
            })
            .map_err(|reject| {
                tracing::info!(
                    client_id = %acts::export::safe_text(&client_id, MAX_FIELD_BYTES),
                    subject = %chan_id,
                    reason = reject.reason(),
                    "subscription refused"
                );
                self.count_rejected(reject.reason());
                self.count_request(acts::ACTION_SUBSCRIBE, "rejected");
                Status::resource_exhausted(format!("subscription refused: {}", reject.reason()))
            })?;

        // The log carries the declared client id, bounded, and the channel
        // key — never the options values, which are client-controlled bytes.
        tracing::info!(
            "on_message client={} chan={}",
            acts::export::safe_text(&client_id, MAX_FIELD_BYTES),
            chan_id
        );
        let queue_size = self
            .engine
            .config()
            .get::<GrpcConfig>("grpc")
            .unwrap_or_default()
            .queue_size();
        let (tx, rx) = mpsc::channel::<Result<Message, Status>>(queue_size);
        let chan_options = ChannelOptions {
            r#type: options.r#type.clone(),
            state: options.state.clone(),
            uses: options.uses.clone(),
            id: chan_id.clone(),
            ack: true,
            options: {
                let mut vars = Vars::new();
                for (k, v) in &options.options {
                    vars.set(k, v.clone());
                }
                vars
            },
        };
        let chan = self.engine.channel_with_options(&chan_options);
        // one shared client: the handler clones the `Arc`, not the client (its
        // channel options carry strings and vars a delivery never reads)
        let client = Arc::new(MessageClient {
            addr: addr.to_string(),
            sender: tx,
            options: chan_options,
            chan: Arc::downgrade(&chan),
        });
        chan.on_message(move |e| {
            let client = client.clone();
            async move {
                let message = Message {
                    name: e.name.clone(),
                    seq: e.id.clone(),
                    ack: None,
                    data: match serde_json::to_vec(e.inner()) {
                        Ok(data) => Some(data),
                        Err(err) => {
                            tracing::error!(mid = %e.inner().id, error = %err, "failed to serialize channel message");
                            client.send(Err(Status::internal(format!(
                                "failed to serialize message {}: {err}",
                                e.inner().id
                            ))));
                            return;
                        }
                    },
                };
                client.send(Ok(message));
            }
        });

        // Count the stream as live work for the drain report; the counter is
        // decremented when the wrapper is dropped (client disconnect or the
        // RPC ended — the same point the lease and channel close).
        self.streams.fetch_add(1, Ordering::AcqRel);
        self.count_request(acts::ACTION_SUBSCRIBE, "ok");

        // Dropping the response stream (client disconnect) closes the
        // channel, releases the subscription lease and deregisters the
        // handler registered above.
        let chan_stream = Box::pin(GuardedMessageStream {
            inner: ReceiverStream::new(rx),
            _guard: CloseChannelOnDrop { chan },
            _lease: lease,
            streams: self.streams.clone(),
        });
        Ok(Response::new(chan_stream))
    }

    async fn send(
        &self,
        request: tonic::Request<Message>,
    ) -> Result<tonic::Response<Message>, tonic::Status> {
        let token = Self::bearer_token(&request);
        // The client's own gRPC deadline is honored (the `grpc-timeout`
        // metadata; tonic 0.14 stores it as metadata and exposes no getter),
        // but never past the profile's server-side deadline: `do_action`
        // takes the minimum. An unparsable timeout is ignored — the server
        // still enforces its own.
        let client_timeout = request
            .metadata()
            .get("grpc-timeout")
            .and_then(|value| value.to_str().ok())
            .and_then(parse_grpc_timeout);
        let message = request.into_inner();
        self.do_action(message, token, client_timeout).await
    }
}

/// Parse the `grpc-timeout` metadata value (`<number><unit>`, units
/// `n`/`u`/`m`/`S`/`M`/`H`) into a duration.
fn parse_grpc_timeout(value: &str) -> Option<Duration> {
    let (digits, unit) = value.split_at(value.len().checked_sub(1)?);
    let n: u64 = digits.parse().ok()?;
    match unit {
        "n" => Some(Duration::from_nanos(n)),
        "u" => Some(Duration::from_micros(n)),
        "m" => Some(Duration::from_millis(n)),
        "S" => Some(Duration::from_secs(n)),
        "M" => Some(Duration::from_secs(n * 60)),
        "H" => Some(Duration::from_secs(n * 3600)),
        _ => None,
    }
}

#[derive(Clone)]
pub struct GrpcPlugin {
    /// The serving task, so `shutdown` can join it. Clones share the handle.
    task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// The unary dispatch counter and stream gauge the [`GrpcTransport`]
    /// reports through — the same arcs the serving `GrpcServer` holds.
    inflight: Arc<InflightCounter>,
    streams: Arc<AtomicUsize>,
}

impl GrpcPlugin {
    pub fn new() -> Self {
        Self {
            task: Arc::new(tokio::sync::Mutex::new(None)),
            inflight: Arc::new(InflightCounter::new()),
            streams: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The lifecycle handle this plugin registers with the engine. Kept
    /// reachable from the plugin so the coverage cases can quiesce/drain the
    /// transport directly; the engine's own close path uses its registration.
    #[cfg(test)]
    pub(crate) fn transport_handle(&self) -> GrpcTransport {
        GrpcTransport {
            inflight: self.inflight.clone(),
            streams: self.streams.clone(),
        }
    }

    /// A server sharing this plugin's dispatch counter and stream gauge —
    /// the coverage cases drive it directly (quiesce/drain without a socket).
    #[cfg(test)]
    pub(crate) fn server_for_test(&self, engine: &Engine) -> GrpcServer {
        GrpcServer::with_shared(engine, self.inflight.clone(), self.streams.clone())
    }

    /// The configured bind address, parsed and validated.
    fn address(engine: &Engine) -> acts::Result<std::net::SocketAddr> {
        let config = engine.config();
        let grpc_config = config.get::<GrpcConfig>("grpc").unwrap_or_default();
        let host = grpc_config
            .host
            .clone()
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = grpc_config.port.unwrap_or(10080);
        format!("{host}:{port}").parse().map_err(|e| {
            acts::ActError::Config(format!("invalid gRPC bind address {host}:{port}: {e}"))
        })
    }

    /// Load `[grpc].tls` into a TLS acceptor: server cert + key, and with a
    /// `client_ca` configured, mutual TLS — the handshake itself refuses a
    /// connection whose client certificate is absent or does not chain to
    /// the configured CA.
    fn tls_acceptor(engine: &Engine) -> acts::Result<Option<tokio_rustls::TlsAcceptor>> {
        let config = engine.config();
        let grpc_config: GrpcConfig = config.get("grpc").unwrap_or_default();
        let Some(tls) = grpc_config.tls else {
            return Ok(None);
        };
        use rustls::pki_types::pem::PemObject;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let certs: Vec<_> = rustls::pki_types::CertificateDer::pem_file_iter(&tls.cert)
            .map_err(|e| {
                acts::ActError::Config(format!("cannot open the gRPC TLS cert {}: {e}", tls.cert))
            })?
            .collect::<Result<_, _>>()
            .map_err(|e| {
                acts::ActError::Config(format!("cannot parse the gRPC TLS cert {}: {e}", tls.cert))
            })?;
        if certs.is_empty() {
            return Err(acts::ActError::Config(format!(
                "the gRPC TLS cert {} contains no certificate",
                tls.cert
            )));
        }
        let key = rustls::pki_types::PrivateKeyDer::from_pem_file(&tls.key).map_err(|e| {
            acts::ActError::Config(format!("cannot load the gRPC TLS key {}: {e}", tls.key))
        })?;
        let server_config = match &tls.client_ca {
            Some(ca_path) => {
                let mut roots = rustls::RootCertStore::empty();
                for cert in
                    rustls::pki_types::CertificateDer::pem_file_iter(ca_path).map_err(|e| {
                        acts::ActError::Config(format!(
                            "cannot open the gRPC client CA {ca_path}: {e}"
                        ))
                    })?
                {
                    roots
                        .add(cert.map_err(|e| {
                            acts::ActError::Config(format!(
                                "cannot parse the gRPC client CA {ca_path}: {e}"
                            ))
                        })?)
                        .map_err(|e| {
                            acts::ActError::Config(format!(
                                "cannot load the gRPC client CA {ca_path}: {e}"
                            ))
                        })?;
                }
                let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                    Arc::new(roots),
                    provider.clone(),
                )
                .build()
                .map_err(|e| {
                    acts::ActError::Config(format!("cannot build the gRPC client verifier: {e}"))
                })?;
                rustls::ServerConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .map_err(|e| {
                        acts::ActError::Config(format!("cannot build the gRPC TLS config: {e}"))
                    })?
                    .with_client_cert_verifier(verifier)
                    .with_single_cert(certs, key)
                    .map_err(|e| {
                        acts::ActError::Config(format!("the gRPC TLS cert/key do not match: {e}"))
                    })?
            }
            None => rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| {
                    acts::ActError::Config(format!("cannot build the gRPC TLS config: {e}"))
                })?
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| {
                    acts::ActError::Config(format!("the gRPC TLS cert/key do not match: {e}"))
                })?,
        };
        Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(
            server_config,
        ))))
    }

    /// Strict mode, level-driven (§2.0): a gRPC server reachable from the
    /// network must either speak TLS or sit behind a TLS-terminating proxy
    /// the deployment explicitly acknowledges. `permissive` warns and
    /// starts; `standard` refuses without TLS or the acknowledgement;
    /// `strict` refuses without TLS outright. Refused in `start` BEFORE the
    /// bind — a plaintext listener that later fails to start TLS never
    /// existed.
    fn strict_bind_check(
        addr: std::net::SocketAddr,
        tls: &Option<TlsConfig>,
        plain_behind_proxy: bool,
        level: acts::Level,
    ) -> acts::Result<()> {
        if addr.ip().is_loopback() || tls.is_some() {
            return Ok(());
        }
        match level {
            acts::Level::Permissive => {
                tracing::warn!(
                    addr = %addr,
                    level = "permissive",
                    "serving gRPC on a non-loopback address without TLS; \
                     configure [grpc].tls or raise the level"
                );
                Ok(())
            }
            // the acknowledgement is exactly the standard level's escape hatch
            acts::Level::Standard if plain_behind_proxy => Ok(()),
            acts::Level::Standard => Err(acts::ActError::Config(format!(
                "refusing to serve gRPC on {addr} without TLS: configure [grpc].tls, \
                 or set [grpc].plain_behind_proxy = true to acknowledge a TLS-terminating proxy"
            ))),
            acts::Level::Strict => Err(acts::ActError::Config(format!(
                "refusing to serve gRPC on {addr} without TLS under the strict level: \
                 plain_behind_proxy is not enough — configure [grpc].tls"
            ))),
        }
    }
}

/// The lifecycle handle the plugin registers with the engine: quiesce stops
/// admitting unary dispatches (streams keep flowing), drain waits for the
/// unary dispatches plus the live `on_message` streams to finish.
struct GrpcTransport {
    inflight: Arc<InflightCounter>,
    streams: Arc<AtomicUsize>,
}

#[cfg(test)]
impl GrpcTransport {
    /// Admitted unary dispatches right now.
    fn inflight_active(&self) -> usize {
        self.inflight.active()
    }
}

#[async_trait::async_trait]
impl acts::RunningTransport for GrpcTransport {
    fn transport(&self) -> &'static str {
        "grpc"
    }

    async fn quiesce(&self) {
        self.inflight.quiesce();
    }

    async fn drain(&self, deadline: Duration) -> acts::TransportDrainReport {
        // Active work is the unary dispatches the counter tracks plus the
        // live subscription streams (their leases keep slots until the
        // stream drops). Polled, not awaited: both counts are plain atomics.
        let deadline = tokio::time::Instant::now() + deadline;
        loop {
            let active = self.inflight.active() + self.streams.load(Ordering::Acquire);
            if active == 0 {
                return acts::TransportDrainReport {
                    transport: "grpc",
                    active_after: 0,
                    drained: true,
                };
            }
            if tokio::time::Instant::now() >= deadline {
                return acts::TransportDrainReport {
                    transport: "grpc",
                    active_after: active,
                    drained: false,
                };
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Default for GrpcPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ActPlugin for GrpcPlugin {
    /// Validate the bind address; nothing listens yet.
    async fn prepare(&self, engine: &Engine) -> acts::Result<()> {
        Self::address(engine)?;
        Ok(())
    }

    async fn start(&self, engine: &Engine) -> acts::Result<()> {
        let engine = engine.clone();
        let addr = Self::address(&engine)?;
        let shutdown = engine.shutdown_token();
        let grpc_config = engine
            .config()
            .get::<GrpcConfig>("grpc")
            .unwrap_or_default();

        // Strict mode BEFORE the bind: a non-loopback listener must be a TLS
        // one (or explicitly acknowledged as behind a proxy, per the level).
        // A refusal here never leaves a plaintext port open, even for a
        // moment. The section's own level key wins over `[security].level`.
        let level = acts::resolve_level(grpc_config.level.as_deref(), &engine.config());
        Self::strict_bind_check(
            addr,
            &grpc_config.tls,
            grpc_config.plain_behind_proxy,
            level,
        )?;
        let tls_acceptor = Self::tls_acceptor(&engine)?;

        // The listener is bound here, not inside the spawned task: a bind
        // failure (port taken, address unavailable) is a deployment fault that
        // must fail the engine start with its reason, instead of logging and
        // leaving a "running" engine whose gRPC transport never came up.
        let listener = std::net::TcpListener::bind(addr).map_err(|e| {
            acts::ActError::Config(format!("failed to bind gRPC server on {addr}: {e}"))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|e| configure_listener_error(addr, e))?;
        let listener = tokio::net::TcpListener::from_std(listener)
            .map_err(|e| register_listener_error(addr, e))?;
        let tcp = tokio_stream::wrappers::TcpListenerStream::new(listener);

        // Each connection is boxed as `Conn` so one server builder serves
        // both modes; with TLS, the acceptor's handshake (including the
        // client-certificate requirement when `client_ca` is configured)
        // runs per connect, before tonic sees the stream.
        let incoming: Incoming = match tls_acceptor {
            Some(acceptor) => Box::pin(tcp.then(move |conn| {
                let acceptor = acceptor.clone();
                async move {
                    match conn {
                        Ok(conn) => {
                            let peer = conn.peer_addr().ok();
                            acceptor
                                .accept(conn)
                                .await
                                .map(|tls| GrpcConn {
                                    io: Box::new(tls),
                                    peer,
                                })
                                .map_err(|e| {
                                    std::io::Error::other(format!("gRPC TLS handshake failed: {e}"))
                                })
                        }
                        Err(e) => Err(e),
                    }
                }
            })),
            None => Box::pin(tcp.map(|conn| {
                conn.map(|conn| GrpcConn {
                    peer: conn.peer_addr().ok(),
                    io: Box::new(conn),
                })
            })),
        };

        // The serving server shares the plugin's dispatch counter and stream
        // gauge: the registered transport handle drains exactly the work
        // this server admits.
        let server = GrpcServer::with_shared(&engine, self.inflight.clone(), self.streams.clone());

        // Register the lifecycle handle before serving: `Engine::close`
        // quiesces and drains every registered transport before plugin
        // shutdown, so a transport that failed to register would be closed
        // without a drain phase.
        engine.register_transport(Arc::new(GrpcTransport {
            inflight: self.inflight.clone(),
            streams: self.streams.clone(),
        }));

        let handle = tokio::spawn(async move {
            // Explicit wire caps, not framework defaults (8 MiB decode /
            // 16 MiB encode), and HTTP/2 keepalive pings so a half-dead
            // connection dies instead of holding a stream slot. See the
            // comment on the `serve` call below for why the service is
            // served directly instead of through the router.
            let grpc = ActsServiceServer::new(server)
                .max_decoding_message_size(MAX_DECODING_MESSAGE_SIZE)
                .max_encoding_message_size(MAX_ENCODING_MESSAGE_SIZE);
            tracing::info!(addr = %addr, "The gRPC server is now ready to accept connections");
            let serve = Server::builder()
                .http2_keepalive_interval(Some(Duration::from_secs(
                    grpc_config.http2_keepalive_secs(),
                )))
                .http2_keepalive_timeout(Some(Duration::from_secs(
                    grpc_config.http2_keepalive_timeout_secs(),
                )))
                .serve_with_incoming_shutdown(grpc, incoming, shutdown.cancelled_owned());
            if let Err(err) = serve.await {
                tracing::error!(addr = %addr, error = %err, "gRPC server stopped");
            } else {
                tracing::info!(addr = %addr, "gRPC server stopped");
            }
        });
        let mut slot = self.task.lock().await;
        *slot = Some(handle);

        Ok(())
    }

    /// The engine cancels the shutdown token before it calls this; joining the
    /// serving task is what proves the port is released.
    async fn shutdown(&self) -> acts::Result<()> {
        let handle = self.task.lock().await.take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
