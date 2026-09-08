// `tonic::Status` (~176 bytes) is the gRPC error type; boxing it would break the
// service trait, so suppress the large-Result lint.
#![allow(clippy::result_large_err)]

use acts::{ActPlugin, ChannelOptions, Engine, Vars};
use acts_channel::{Message, MessageOptions, acts_service_server::*};
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Response, Status, transport::Server};

pub use config::GrpcConfig;

mod config;

type MessageStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<Message, Status>> + Send>>;

#[derive(Clone)]
struct MessageClient {
    addr: String,
    sender: Sender<Result<Message, Status>>,
    options: ChannelOptions,
}

impl MessageClient {
    fn send(&self, message: Message) {
        let msg = Ok(message);
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

    async fn do_action(&self, message: Message) -> Result<Response<Message>, Status> {
        let options = match message.data {
            Some(ref data) => serde_json::from_slice::<Vars>(data).unwrap_or_default(),
            None => Vars::new(),
        };
        tracing::info!(
            "do-action seq={} name={} ack={:?} options={options}",
            message.seq,
            message.name,
            message.ack
        );

        let name = message.name.clone();
        let value = match acts::actions::apply(&self.engine, &name, options).await {
            Ok(value) => value,
            Err(acts::actions::Error::NotFound(msg)) => {
                return Err(Status::not_found(msg));
            }
            Err(acts::actions::Error::Invalid(msg)) => {
                return Err(Status::invalid_argument(msg));
            }
            Err(acts::actions::Error::Internal(msg)) => {
                tracing::error!("do-action err={msg}");
                return Err(Status::new(Code::Internal, msg));
            }
        };

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
        let (tx, rx) = mpsc::channel::<Result<Message, Status>>(128);
        let addr = req.remote_addr().unwrap();
        let options = req.into_inner();

        tracing::info!("on_message: options={options:?}");
        let client = MessageClient {
            addr: addr.to_string(),
            sender: tx,
            options: ChannelOptions {
                r#type: options.r#type.clone(),
                state: options.state.clone(),
                uses: options.uses.clone(),
                id: options.client_id.clone(),
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
        tokio::spawn(async move {
            chan.on_message(move |e| {
                let client = client.clone();
                async move {
                    let message = Message {
                        name: e.name.clone(),
                        seq: e.id.clone(),
                        ack: None,
                        data: Some(serde_json::to_vec(e.inner()).unwrap()),
                    };
                    client.send(message);
                }
            });
        });

        let chan_stream = Box::pin(ReceiverStream::new(rx));
        Ok(Response::new(chan_stream))
    }

    async fn send(
        &self,
        request: tonic::Request<Message>,
    ) -> Result<tonic::Response<Message>, tonic::Status> {
        self.do_action(request.into_inner()).await
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
        let addr = format!("0.0.0.0:{port}");

        tokio::spawn(async move {
            let addr = addr.parse().unwrap();
            let server = GrpcServer::new(&engine);
            let grpc = ActsServiceServer::new(server);
            println!(
                "The gRPC server is now ready to accept connections on port {}",
                port
            );
            Server::builder()
                .add_service(grpc)
                .serve(addr)
                .await
                .unwrap();
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests;
