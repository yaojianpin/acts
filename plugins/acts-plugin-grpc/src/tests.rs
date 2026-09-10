use crate::{GrpcConfig, GrpcPlugin, GrpcServer};
use acts::Engine;
use serde_json::json;

#[test]
fn test_grpc_config_default() {
    let config = GrpcConfig::default();
    assert_eq!(config.port, None);
}

#[test]
fn test_grpc_config_deserialize() {
    let json = json!({"port": 9999});
    let config: GrpcConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.port, Some(9999));
}

#[test]
fn test_grpc_config_deserialize_empty() {
    let json = json!({});
    let config: GrpcConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.port, None);
}

#[test]
fn test_grpc_plugin_new() {
    let plugin = GrpcPlugin::new();
    let _ = plugin; // Ensure construction succeeds
}

#[test]
fn test_grpc_plugin_default() {
    let plugin = GrpcPlugin;
    let _ = plugin;
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn engine_with_grpc(port: u16) -> Engine {
    let table: toml::Table = toml::from_str(&format!("[grpc]\nport = {port}\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    Engine::builder()
        .set_config(&cfg)
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_snapshot_upsert_remove_over_grpc() {
    use acts_channel::{ActsChannel, Vars};

    let port = free_port();
    let engine = engine_with_grpc(port).await;

    // connect the client and wait until the server accepts
    let url = format!("http://127.0.0.1:{port}");
    let mut client = loop {
        match ActsChannel::connect(&url).await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    };

    let ok = client
        .upsert_snapshot("profile", "u1", 5, Vars::new().with("val", "x"))
        .await
        .unwrap();
    assert_eq!(ok.data, Some(true));
    let entry = engine.snapshot().read("profile", "u1").unwrap();
    assert_eq!(entry.rev, 5);
    assert_eq!(entry.data.get::<String>("val").unwrap(), "x");

    let ok = client.remove_snapshot("profile", "u1").await.unwrap();
    assert_eq!(ok.data, Some(true));
    assert!(engine.snapshot().read("profile", "u1").is_none());

    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_grpc_server_new() {
    let engine = Engine::builder().start().await.unwrap();
    let server = GrpcServer::new(&engine);
    let _ = server;
    engine.close().await;
}
