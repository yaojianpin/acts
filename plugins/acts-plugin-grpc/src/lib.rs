// `tonic::Status` (~176 bytes) is the gRPC error type; boxing it would break the
// service trait, so suppress the large-Result lint.
#![allow(clippy::result_large_err)]

use acts::{ActPlugin, Channel, ChannelOptions, Engine, Vars};
use acts_channel::{Message, MessageOptions, acts_service_server::*};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tonic::{Code, Response, Status, transport::Server};

pub use config::GrpcConfig;

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

#[derive(Clone)]
struct MessageClient {
    addr: String,
    sender: Sender<Result<Message, Status>>,
    options: ChannelOptions,
}

impl MessageClient {
    fn send(&self, message: Result<Message, Status>) {
        let msg = message;
        let client = self.clone();
        if client.sender.is_closed() {
            tracing::warn!("client {}({}) is closed", client.addr, client.options.id);
            return;
        }
        tokio::spawn(async move {
            match client.sender.send(msg).await {
                Ok(_) => {
                    tracing::info!("send to {}({})", client.addr, client.options.id);
                }
                Err(err) => {
                    tracing::error!(
                        "send to {}({}), error={:?}",
                        client.addr,
                        client.options.id,
                        err
                    );
                }
            }
        });
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
            seq: acts_channel::create_seq(),
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
        let (tx, rx) = mpsc::channel::<Result<Message, Status>>(128);
        let client = MessageClient {
            addr: addr.to_string(),
            sender: tx,
            options: ChannelOptions {
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
            },
        };
        let chan = self.engine.channel_with_options(&client.options);
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
            let serve = Server::builder()
                .add_service(grpc)
                .serve_with_shutdown(addr, shutdown.cancelled_owned());
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
