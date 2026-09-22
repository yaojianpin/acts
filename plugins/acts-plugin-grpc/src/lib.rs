// `tonic::Status` (~176 bytes) is the gRPC error type; boxing it would break the
// service trait, so suppress the large-Result lint.
#![allow(clippy::result_large_err)]

use acts::{ActPlugin, Channel, ChannelOptions, Engine, Vars};
use acts_proto::{Message, MessageOptions, acts_service_server::*};
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use tokio::sync::mpsc::{self, Sender, error::TrySendError};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tonic::{Code, Response, Status, transport::Server};

pub use config::{DEFAULT_QUEUE_SIZE, GrpcConfig};

mod config;

type MessageStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<Message, Status>> + Send>>;

/// Map an action-dispatch failure onto its gRPC status, so the unary and the
/// subscription paths answer a refusal the same way.
fn action_status(err: acts::actions::Error) -> Status {
    match err {
        acts::actions::Error::NotFound(msg) => Status::not_found(msg),
        acts::actions::Error::Invalid(msg) => Status::invalid_argument(msg),
        acts::actions::Error::Unauthenticated(msg) => Status::unauthenticated(msg),
        acts::actions::Error::Denied(msg) => Status::permission_denied(msg),
        acts::actions::Error::Internal(msg) => {
            tracing::error!("do-action err={msg}");
            Status::new(Code::Internal, msg)
        }
    }
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
/// stream is dropped by the transport.
struct GuardedMessageStream {
    inner: ReceiverStream<Result<Message, Status>>,
    _guard: CloseChannelOnDrop,
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
}

impl GrpcServer {
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
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

    async fn do_action(
        &self,
        message: Message,
        token: Option<String>,
    ) -> Result<Response<Message>, Status> {
        let options = match &message.data {
            // `data` is the action payload and MUST be a JSON object. A
            // malformed payload is a caller error, never an empty option set:
            // silently defaulting to empty options would run the action's
            // global branch (e.g. `msg:clear` clearing every error delivery)
            // instead of rejecting the request.
            Some(data) => serde_json::from_slice::<Vars>(data)
                .map_err(|err| Status::invalid_argument(format!("invalid message data: {err}")))?,
            None => Vars::new(),
        };
        tracing::info!(
            "do-action seq={} name={} ack={:?} authenticated={}",
            message.seq,
            message.name,
            message.ack,
            token.is_some()
        );

        let principal = match self.engine.acl().authenticate(token.as_deref()) {
            Ok(principal) => principal,
            Err(err) => return Err(Status::unauthenticated(err.to_string())),
        };

        let name = message.name.clone();
        let value = acts::actions::apply_as(&self.engine, &principal, &name, options)
            .await
            .map_err(action_status)?;

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
        let principal = self
            .engine
            .acl()
            .authenticate(token.as_deref())
            .map_err(|err| Status::unauthenticated(err.to_string()))?;
        let addr = req
            .remote_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let options = req.into_inner();
        let chan_id = acts::actions::apply_as(
            &self.engine,
            &principal,
            acts::ACTION_SUBSCRIBE,
            Vars::new().with("client_id", options.client_id.clone()),
        )
        .await
        .map_err(action_status)?
        .as_str()
        .unwrap_or_default()
        .to_string();

        tracing::info!("on_message: options={options:?} chan={chan_id}");
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

        // Dropping the response stream (client disconnect) closes the
        // channel and deregisters the handler registered above.
        let chan_stream = Box::pin(GuardedMessageStream {
            inner: ReceiverStream::new(rx),
            _guard: CloseChannelOnDrop { chan },
        });
        Ok(Response::new(chan_stream))
    }

    async fn send(
        &self,
        request: tonic::Request<Message>,
    ) -> Result<tonic::Response<Message>, tonic::Status> {
        let token = Self::bearer_token(&request);
        self.do_action(request.into_inner(), token).await
    }
}

#[derive(Clone)]
pub struct GrpcPlugin;

impl GrpcPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrpcPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ActPlugin for GrpcPlugin {
    fn on_init(&self, engine: &Engine) -> acts::Result<()> {
        let engine = engine.clone();
        let config = engine.config();
        let grpc_config = config.get::<GrpcConfig>("grpc").unwrap_or_default();
        let port = grpc_config.port.unwrap_or(10080);
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port as u16));
        let shutdown = engine.shutdown_token();

        tokio::spawn(async move {
            let server = GrpcServer::new(&engine);
            let grpc = ActsServiceServer::new(server);
            println!(
                "The gRPC server is now ready to accept connections on port {}",
                port
            );
            // tonic 0.14's `add_service` is part of its `router` feature (an
            // axum 0.8 dependency): it exists to route several named services.
            // This plugin serves exactly one, and the generated
            // `ActsServiceServer` already dispatches the gRPC paths itself and
            // answers `UNIMPLEMENTED` to anything else — the same thing the
            // router's fallback does — so the service goes to `serve` directly
            // and the transport stays free of the HTTP framework.
            let serve =
                Server::builder().serve_with_shutdown(addr, grpc, shutdown.cancelled_owned());
            if let Err(err) = serve.await {
                tracing::error!(addr = %addr, error = %err, "gRPC server stopped");
            } else {
                tracing::info!(addr = %addr, "gRPC server stopped");
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests;
