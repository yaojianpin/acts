use acts::{ActPlugin, ChannelOptions, Engine, Vars, Workflow};
use acts_channel::{Message, MessageOptions, acts_service_server::*};
use serde_json::Value;
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Response, Status, transport::Server};

pub use config::GrpcConfig;

mod config;

type MessageStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<Message, Status>> + Send>>;

macro_rules! wrap_result {
    ($seq:expr, $name:expr, $input:expr) => {
        match $input {
            Ok(data) => {
                let mut message = wrap_message($name, &data);
                message.ack = Some($seq.to_string());
                Ok(Response::new(message))
            }
            Err(err) => {
                tracing::error!("wrap_result err= {err:?}");
                Err(Status::new(Code::Internal, err.to_string()))
            }
        }
    };
}

fn wrap_message<T: ?Sized + serde::Serialize>(name: &str, value: &T) -> Message {
    Message {
        name: name.to_string(),
        seq: acts_channel::create_seq(),
        ack: None,
        data: Some(serde_json::to_vec(value).unwrap()),
    }
}

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

        let name = message.name.as_str();
        let ack = message.seq.as_str();
        let executor = self.engine.executor();
        match name {
            // act
            "act:push" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().push(&pid, &tid, options))
            }
            "act:remove" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().remove(&pid, &tid, options))
            }
            "act:submit" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().submit(&pid, &tid, options))
            }
            "act:complete" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().complete(&pid, &tid, options))
            }
            "act:abort" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().abort(&pid, &tid, options))
            }
            "act:cancel" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().cancel(&pid, &tid, options))
            }
            "act:back" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().back(&pid, &tid, options))
            }
            "act:skip" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().skip(&pid, &tid, options))
            }
            "act:error" => {
                let mut options = options;
                let pid = options
                    .pop::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .pop::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.act().fail(&pid, &tid, options))
            }
            // model
            "model:ls" => {
                let query = options
                    .get::<acts::query::Query>("query")
                    .unwrap_or(acts::query::Query::new().limit(100));
                wrap_result!(ack, name, executor.model().list(&query))
            }
            "model:rm" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.model().rm(&id))
            }
            "model:get" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                let fmt = options.get::<String>("fmt").unwrap_or("text".to_string());
                wrap_result!(ack, name, executor.model().get(&id, &fmt))
            }
            "model:deploy" => {
                let model_text = options
                    .get::<String>("model")
                    .ok_or(Status::invalid_argument("model is required"))?;
                let mut model = Workflow::from_yml(&model_text)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?;
                if let Some(mid) = options.get::<String>("mid") {
                    model.set_id(&mid);
                }
                wrap_result!(ack, name, executor.model().deploy(&model, None))
            }
            // package
            "pack:ls" => {
                let query = options
                    .get::<acts::query::Query>("query")
                    .unwrap_or(acts::query::Query::new().limit(100));
                wrap_result!(ack, name, executor.pack().list(&query))
            }
            "pack:get" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.pack().get(&id))
            }
            "pack:publish" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("package 'id' is required"))?;
                let pack_name = options.get::<String>("name").unwrap_or_default();
                let desc = options.get::<String>("desc").unwrap_or_default();
                let icon = options.get::<String>("icon").unwrap_or_default();
                let doc = options.get::<String>("doc").unwrap_or_default();
                let version = options.get::<String>("version").unwrap_or_default();
                let schema = options
                    .get::<serde_json::Value>("schema")
                    .unwrap_or_default();
                let pack_options = options
                    .get::<Option<serde_json::Value>>("options")
                    .unwrap_or_default();
                let run_as = options.get::<String>("run_as").unwrap_or_default();
                let resources = options
                    .get::<Vec<acts::ActResource>>("resources")
                    .unwrap_or_default();
                let catalog = options.get::<String>("catalog").unwrap_or_default();
                let pack = acts::data::Package {
                    id,
                    name: pack_name,
                    desc,
                    icon,
                    doc,
                    version,
                    schema: schema.to_string(),
                    options: pack_options.map(|v| v.to_string()),
                    run_as: std::str::FromStr::from_str(&run_as)
                        .map_err(|_err| Status::invalid_argument("package 'run_as' is invalid"))?,
                    resources: serde_json::to_string(&resources).map_err(|err| {
                        Status::invalid_argument(format!("package 'resource' error: {}", err))
                    })?,
                    catalog: std::str::FromStr::from_str(&catalog)
                        .map_err(|_err| Status::invalid_argument("package 'catalog' is invalid"))?,
                    ..Default::default()
                };
                wrap_result!(ack, name, executor.pack().publish(&pack))
            }
            "pack:rm" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.pack().rm(&id))
            }
            // proc
            "proc:start" => {
                let mut options = options;
                let id = options
                    .pop::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.proc().start(&id, options))
            }
            "proc:start_from_model" => {
                let mut options = options;
                let fmt = options
                    .pop::<String>("fmt")
                    .ok_or(Status::invalid_argument("fmt is required"))?;
                let model = options
                    .pop::<String>("model")
                    .ok_or(Status::invalid_argument("model is required"))?;
                wrap_result!(
                    ack,
                    name,
                    executor.proc().start_from_model(&model, &fmt, options)
                )
            }
            "proc:ls" => {
                let query = options
                    .get::<acts::query::Query>("query")
                    .unwrap_or(acts::query::Query::new().limit(100));
                wrap_result!(ack, name, executor.proc().list(&query))
            }
            "proc:get" => {
                let pid = options
                    .get::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                wrap_result!(ack, name, executor.proc().get(&pid))
            }
            // task
            "task:ls" => {
                let query = options
                    .get::<acts::query::Query>("query")
                    .unwrap_or(acts::query::Query::new().limit(100));
                wrap_result!(ack, name, executor.task().list(&query))
            }
            "task:get" => {
                let pid = options
                    .get::<String>("pid")
                    .ok_or(Status::invalid_argument("pid is required"))?;
                let tid = options
                    .get::<String>("tid")
                    .ok_or(Status::invalid_argument("tid is required"))?;
                wrap_result!(ack, name, executor.task().get(&pid, &tid))
            }
            // msg
            "msg:ls" => {
                let query = options
                    .get::<acts::query::Query>("query")
                    .unwrap_or(acts::query::Query::new().limit(100));
                wrap_result!(ack, name, executor.msg().list(&query))
            }
            "msg:get" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.msg().get(&id))
            }
            "msg:ack" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.msg().ack(&id))
            }
            "msg:redo" => {
                wrap_result!(ack, name, executor.msg().redo())
            }
            "msg:clear" => {
                let pid = options.get::<String>("pid");
                wrap_result!(ack, name, executor.msg().clear(pid))
            }
            "msg:rm" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.msg().rm(&id))
            }
            "msg:unsub" => {
                let client_id = options
                    .get::<String>("client_id")
                    .ok_or(Status::invalid_argument("client id is required"))?;
                wrap_result!(ack, name, executor.msg().unsub(&client_id))
            }
            // event
            "evt:ls" => {
                let query = options
                    .get::<acts::query::Query>("query")
                    .unwrap_or(acts::query::Query::new().limit(100));
                wrap_result!(ack, name, executor.evt().list(&query))
            }
            "evt:get" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("id is required"))?;
                wrap_result!(ack, name, executor.evt().get(&id))
            }
            "evt:start" => {
                let id = options
                    .get::<String>("id")
                    .ok_or(Status::invalid_argument("event id is required"))?;
                let params = options.get::<Value>("params").unwrap_or_default();
                wrap_result!(ack, name, executor.evt().start(&id, &params))
            }
            _ => Err(Status::not_found(format!("not found action '{name}'"))),
        }
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
                let message = Message {
                    name: e.name.clone(),
                    seq: e.id.clone(),
                    ack: None,
                    data: Some(serde_json::to_vec(e.inner()).unwrap()),
                };
                client.send(message);
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
