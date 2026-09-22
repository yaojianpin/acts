use std::time::Duration;

use acts::{Config, Engine};
use acts_plugin_grpc::GrpcPlugin;
use futures::StreamExt;
use tokio::sync::oneshot::{self, Receiver};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use crate::acts_service_server::{ActsService, ActsServiceServer};

mod act;
mod subscribe;
mod vars;
mod workflow;

#[cfg(feature = "docker_test")]
pub const SERVER_ADDR: &str = "172.17.0.1";

#[cfg(not(feature = "docker_test"))]
pub const SERVER_ADDR: &str = "127.0.0.1";

/// How long a test waits for the server it just started to accept a
/// connection. The gRPC plugin binds its listener in a spawned task, and CI
/// runs this suite on a shared runner, so the budget is generous: waiting it
/// out means something is actually wrong.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

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
        // tonic 0.14's service router (`add_service`) is behind its `router`
        // feature and routes by service name; one service needs no router, and
        // `serve_with_incoming` takes it directly: the generated
        // `ActsServiceServer` dispatches the gRPC paths itself.
        Server::builder()
            .serve_with_incoming(grpc, incoming)
            .await
            .unwrap();
    });

    // Wait until the server task has started
    ready_rx.await.ok();
    port
}

/// Start a real server for the client to talk to: the shipped engine with the
/// shipped gRPC plugin, on a port picked for this test. Returns that port; the
/// engine (and with it the listener) is closed when `rx` fires.
///
/// What used to stand here was a second implementation of the gRPC service —
/// its own action table for every `act:`/`model:`/`pack:` name, kept in step
/// with the shipped one by hand.
async fn start_server(rx: Receiver<()>) -> u16 {
    let dir = std::env::temp_dir().join(format!(
        "acts-channel-{}-{}",
        std::process::id(),
        crate::create_seq()
    ));
    std::fs::create_dir_all(&dir).expect("a scratch directory for the test server");
    let port = free_port();
    let config_path = dir.join("acts.toml");
    // `[acl] enabled = false`: these cases exercise the transport, and an
    // engine *without* a section is anonymous and read-only (models and
    // packages only) — the deploy/start cases need more than that.
    std::fs::write(
        &config_path,
        format!("[acl]\nenabled = false\n[grpc]\nport = {port}\n"),
    )
    .expect("the test server's config file");
    let config = Config::create(&config_path).expect("the test server's config");

    let engine = Engine::builder()
        .set_config(&config)
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .expect("the test server's engine");
    wait_for_server(port).await;

    tokio::spawn(async move {
        rx.await.ok();
        engine.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    });

    port
}

/// A port nothing is listening on: bound and released, so the gRPC plugin can
/// take it.
fn free_port() -> u16 {
    let listener =
        std::net::TcpListener::bind(format!("{SERVER_ADDR}:0")).expect("a port for the server");
    let port = listener.local_addr().expect("the bound address").port();
    drop(listener);
    port
}

/// Wait until the server accepts a connection: the gRPC plugin binds its
/// listener in a spawned task, so the first attempts may still be refused.
async fn wait_for_server(port: u16) {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::net::TcpStream::connect((SERVER_ADDR, port)).await {
            Ok(_) => return,
            Err(err) => last = err.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the server never accepted a connection on {SERVER_ADDR}:{port}: {last}");
}
