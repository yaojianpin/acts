use crate::{GrpcConfig, GrpcPlugin, GrpcServer};
use acts::query::Query as StoreQuery;
use acts::{ChannelOptions, Engine, Vars, Workflow};
use acts_channel::{MessageOptions, acts_service_server::ActsService};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

async fn run_irq_workflow(engine: &Engine, key: &str) {
    let model = Workflow::new()
        .with_id(&format!("grpc-leak-{key}"))
        .with_step(|step| {
            step.with_id("step1")
                .with_uses("acts.core.irq", Vars::new().with("key", "leak-test"))
        });
    engine
        .executor()
        .model()
        .deploy(&model, None)
        .await
        .unwrap();
    engine
        .executor()
        .proc()
        .start(&model.id, Vars::new())
        .await
        .unwrap();
}

async fn stored_message_count(engine: &Engine) -> usize {
    engine
        .executor()
        .msg()
        .list(&StoreQuery::new().offset(0).limit(1000))
        .await
        .unwrap()
        .count
}

async fn wait_until(mut cond: impl FnMut() -> bool, label: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {label}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// When the gRPC response stream is dropped (client disconnected), the
/// channel handler must be deregistered. Message rows are only written by
/// ack-channel deliveries, so the stored message count is the leak detector:
/// a leaked handler would store one message row per workflow message forever.
#[tokio::test(flavor = "multi_thread")]
async fn test_on_message_stream_drop_deregisters_channel() {
    let engine = Engine::builder().start().await.unwrap();
    let server = GrpcServer::new(&engine);

    // spy channel: counts every dispatched workflow message without
    // storing anything (ack = false)
    let received = Arc::new(Mutex::new(0usize));
    let spy = engine.channel_with_options(&ChannelOptions {
        ack: false,
        ..Default::default()
    });
    let spy_received = received.clone();
    spy.on_message(move |_e| {
        let received = spy_received.clone();
        async move {
            *received.lock().unwrap() += 1;
        }
    });

    // subscribe like a gRPC client would
    let options = MessageOptions {
        client_id: "leak-test-client".to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    };
    let response = server
        .on_message(tonic::Request::new(options))
        .await
        .unwrap();

    // positive control: while the stream is alive, its ack channel stores
    // one message row per workflow message
    run_irq_workflow(&engine, "alive").await;
    wait_until(
        || *received.lock().unwrap() >= 3,
        "workflow messages to be dispatched",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let alive_rows = stored_message_count(&engine).await;
    assert!(alive_rows >= 1, "live channel should store deliveries");

    // client disconnects: tonic drops the response stream, which must
    // deregister the channel handler
    drop(response);

    // new messages must not be stored for the dead channel anymore
    run_irq_workflow(&engine, "dropped").await;
    wait_until(
        || *received.lock().unwrap() >= 6,
        "messages of the second workflow to be dispatched",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        stored_message_count(&engine).await,
        alive_rows,
        "dropped gRPC channel must not store further deliveries"
    );

    engine.close().await;
}
