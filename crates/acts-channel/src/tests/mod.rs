use std::sync::Arc;

use acts::Engine;
use futures::StreamExt;
use tokio::sync::oneshot::{self, Receiver};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use crate::acts_service_server::{ActsService, ActsServiceServer};

mod act;
mod server;
mod subscribe;
mod vars;
mod workflow;

#[cfg(feature = "docker_test")]
pub const SERVER_ADDR: &str = "172.17.0.1";

#[cfg(not(feature = "docker_test"))]
pub const SERVER_ADDR: &str = "127.0.0.1";

/// Start a test gRPC server for `service` on an OS-assigned port.
/// Blocks until the server is accepting connections, then returns the port.
/// The server shuts down when `rx` fires.
async fn serve_service<S: ActsService>(service: S, rx: Receiver<()>) -> u16 {
    let grpc = ActsServiceServer::new(service);

    // Bind to port 0 to let the OS pick a free port
    let listener = tokio::net::TcpListener::bind(format!("{SERVER_ADDR}:0"))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();

    let incoming = TcpListenerStream::new(listener);
    // Shut down the incoming stream when rx fires
    let incoming = incoming.take_until(async move {
        rx.await.ok();
    });

    let (ready_tx, ready_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        ready_tx.send(()).ok();
        Server::builder()
            .add_service(grpc)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    // Wait until the server task has started
    ready_rx.await.ok();
    port
}

/// Start the acts gRPC test server: the real `GrpcServer` over an `Engine`.
async fn start_server(rx: Receiver<()>) -> u16 {
    let engine = Arc::new(Engine::builder().start().await.unwrap());
    let server = server::GrpcServer::new(engine);
    server.init().await;
    serve_service(server, rx).await
}
