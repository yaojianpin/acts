use std::sync::Arc;

use acts::Engine;
use futures::StreamExt;
use tokio::sync::oneshot::{self, Receiver};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use crate::acts_service_server::ActsServiceServer;

mod act;
mod server;
mod workflow;

#[cfg(feature = "docker_test")]
pub const SERVER_ADDR: &str = "172.17.0.1";

#[cfg(not(feature = "docker_test"))]
pub const SERVER_ADDR: &str = "127.0.0.1";

/// Start a gRPC test server on an OS-assigned port.
/// Blocks until the server is accepting connections, then returns the port.
async fn start_server(rx: Receiver<()>) -> u16 {
    let engine = Arc::new(Engine::new().start().await.unwrap());
    let server = server::GrpcServer::new(engine);
    server.init().await;
    let grpc = ActsServiceServer::new(server);

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
