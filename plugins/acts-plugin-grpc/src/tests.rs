use crate::{GrpcConfig, GrpcPlugin, GrpcServer, wrap_message};
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
fn test_wrap_message_basic() {
    let msg = wrap_message("test.event", "hello");
    assert_eq!(msg.name, "test.event");
    assert!(!msg.seq.is_empty());
    assert_eq!(msg.ack, None);
    let data = msg.data.unwrap();
    let text = String::from_utf8(data).unwrap();
    assert_eq!(text, "\"hello\"");
}

#[test]
fn test_wrap_message_json() {
    let value = json!({"key": "val", "num": 42});
    let msg = wrap_message("data.event", &value);
    assert_eq!(msg.name, "data.event");
    let data = msg.data.unwrap();
    let back: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(back, value);
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

#[test]
fn test_grpc_server_new() {
    let engine = Engine::new();
    let server = GrpcServer::new(&engine);
    let _ = server;
}
