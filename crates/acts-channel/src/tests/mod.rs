use std::sync::LazyLock;
use std::time::Duration;

use acts::{Config, Engine};
use acts_acl::AclUsers;
use acts_plugin_grpc::GrpcPlugin;
use futures::StreamExt;
use tokio::sync::oneshot::{self, Receiver};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use crate::acts_service_server::{ActsService, ActsServiceServer};

mod act;
mod auth;
mod subscribe;
mod vars;
mod workflow;

/// The address the test servers bind and the clients dial: loopback, unless
/// `SERVER_ADDR` says otherwise — the Docker-bridge gateway (`172.17.0.1`)
/// where a Linux CI job wants the transport exercised off loopback. An env
/// var, not a compile-time feature: an address baked in at build time is
/// right only on the host shape it was baked for, so `--all-features` on a
/// Windows box bound the Linux bridge and failed every case with
/// `AddrNotAvailable (10049)`.
///
/// The first use is the health check: the value must be an IP address and
/// accept a bind, so a bad setting stops the case that hit it — at once, with
/// the reason — instead of surfacing as a bare `AddrNotAvailable` out of every
/// helper, or as a refused connection after the full ready timeout.
pub fn server_addr() -> &'static str {
    static ADDR: LazyLock<String> = LazyLock::new(|| {
        let addr = std::env::var("SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1".into());
        if let Err(err) = addr.parse::<std::net::IpAddr>() {
            panic!(
                "SERVER_ADDR={addr} is not an IP address: {err}; the test servers bind it directly"
            );
        }
        if let Err(err) = std::net::TcpListener::bind((addr.as_str(), 0)) {
            panic!(
                "SERVER_ADDR={addr} is not bindable on this host: {err}; unset SERVER_ADDR or point it at a local address"
            );
        }
        addr
    });
    ADDR.as_str()
}

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
    let listener = tokio::net::TcpListener::bind(format!("{}:0", server_addr()))
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

/// Start the shipped engine with the shipped gRPC plugin on a port picked for
/// this test. `disable_acl` selects the ACL the engine runs under
/// ([`EngineBuilder::disable_acl`](acts::EngineBuilder::disable_acl) makes
/// every caller unrestricted; otherwise the store-backed user registry is
/// installed with `with_user_acl`, so a case can declare users); the engine is
/// closed, and its scratch directory removed, when `rx` fires.
///
/// What used to stand here was a second implementation of the gRPC service —
/// its own action table for every `act:`/`model:`/`pack:` name, kept in step
/// with the shipped one by hand.
async fn serve(rx: Receiver<()>, disable_acl: bool) -> (Engine, u16) {
    let dir = std::env::temp_dir().join(format!(
        "acts-channel-{}-{}",
        std::process::id(),
        crate::create_seq()
    ));
    std::fs::create_dir_all(&dir).expect("a scratch directory for the test server");
    let addr = server_addr();
    let port = free_port();
    let config_path = dir.join("acts.toml");
    // `[grpc] host` follows `server_addr()`: the plugin must bind the address
    // the client dials (its own default is loopback only), or the bridge
    // setting dials an interface nothing listens on. The ACL is chosen on the
    // builder, not by a config section: `[acl]` is no longer read, and an
    // enabled ACL refuses the deploy/start these cases exercise to an
    // unauthenticated caller.
    std::fs::write(
        &config_path,
        format!("[grpc]\nhost = \"{addr}\"\nport = {port}\n"),
    )
    .expect("the test server's config file");
    let config = Config::create(&config_path).expect("the test server's config");

    let mut builder = Engine::builder()
        .set_config(&config)
        .add_plugin(&GrpcPlugin::new());
    // The user registry is a crate of its own, installed on the builder: the
    // enabled case gets a registry it can declare users in, and the disabled
    // case opts the ACL out entirely.
    if disable_acl {
        builder = builder.disable_acl();
    } else {
        builder = builder.with_user_acl();
    }
    let engine = builder.start().await.expect("the test server's engine");
    wait_for_server(port).await;

    let closing = engine.clone();
    tokio::spawn(async move {
        rx.await.ok();
        closing.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    });

    (engine, port)
}

/// The transport cases' server: the ACL is disabled, so every caller is
/// unrestricted and no credential is needed.
async fn start_server(rx: Receiver<()>) -> u16 {
    serve(rx, true).await.1
}

/// A server with the ACL *enabled* — the user model, not the disabled-ACL
/// shortcut. The engine comes back so a case can declare users on it
/// (`engine.acl().set_user(…)`) before it dials and logs in.
async fn start_auth_server(rx: Receiver<()>) -> (Engine, u16) {
    serve(rx, false).await
}

/// A port nothing is listening on: bound and released, so the gRPC plugin can
/// take it.
fn free_port() -> u16 {
    let listener =
        std::net::TcpListener::bind(format!("{}:0", server_addr())).expect("a port for the server");
    let port = listener.local_addr().expect("the bound address").port();
    drop(listener);
    port
}

/// Wait until the server accepts a connection: the gRPC plugin binds its
/// listener in a spawned task, so the first attempts may still be refused.
async fn wait_for_server(port: u16) {
    let addr = server_addr();
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::net::TcpStream::connect((addr, port)).await {
            Ok(_) => return,
            Err(err) => last = err.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the server never accepted a connection on {addr}:{port}: {last}");
}
