use crate::{
    DEFAULT_QUEUE_SIZE, HttpConfig, WebPlugin,
    objects::{AppError, RespData, RespStatus},
};
use axum::response::IntoResponse;
use serde_json::json;

// ── HttpConfig ──

#[test]
fn test_http_config_default() {
    let config = HttpConfig::default();
    assert_eq!(config.port, None);
    assert_eq!(
        config.queue_size(),
        DEFAULT_QUEUE_SIZE,
        "an unconfigured subscription queue keeps the default capacity"
    );
}

/// The configured queue capacity is what the subscription is built with, and
/// `0` is clamped to a queue of one: a zero-capacity queue would open a stream
/// that can never receive a message.
#[test]
fn test_http_config_queue_size_is_honored_and_clamped() {
    let config: HttpConfig = serde_json::from_value(json!({"queue_size": 8})).unwrap();
    assert_eq!(config.queue_size(), 8);

    let config: HttpConfig = serde_json::from_value(json!({"queue_size": 0})).unwrap();
    assert_eq!(config.queue_size(), 1);
}

#[test]
fn test_http_config_deserialize() {
    let json = json!({"port": 8080});
    let config: HttpConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.port, Some(8080));
}

#[test]
fn test_http_config_deserialize_empty() {
    let json = json!({});
    let config: HttpConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.port, None);
}

// ── RespData ──

#[test]
fn test_resp_data_ok() {
    let resp = RespData::ok("success");
    assert_eq!(resp.code, RespStatus::Ok);
    assert_eq!(resp.data, Some("success"));
    assert_eq!(resp.message, None);
    assert_eq!(resp.details, None);
}

#[test]
fn test_resp_data_err() {
    let resp: RespData<()> = RespData::err("something went wrong");
    assert_eq!(resp.code, RespStatus::Error);
    assert_eq!(resp.data, None);
    assert_eq!(resp.message.as_deref(), Some("something went wrong"));
    assert_eq!(resp.details, None);
}

#[test]
fn test_resp_data_err_with_details() {
    let resp: RespData<()> = RespData::err_with_details("fail", "stack trace");
    assert_eq!(resp.code, RespStatus::Error);
    assert_eq!(resp.data, None);
    assert_eq!(resp.message.as_deref(), Some("fail"));
    assert_eq!(resp.details.as_deref(), Some("stack trace"));
}

#[test]
fn test_resp_data_ok_json_value() {
    let data = json!({"id": 1, "name": "test"});
    let resp = RespData::ok(data.clone());
    assert_eq!(resp.code, RespStatus::Ok);
    assert_eq!(resp.data, Some(data));
}

#[test]
fn test_resp_status_values() {
    assert_eq!(RespStatus::Ok as i32, 200);
    assert_eq!(RespStatus::Error as i32, 500);
}

// ── AppError ──

#[test]
fn test_app_error_from_str_into_response() {
    let err = AppError::from("bad input");
    let resp = err.into_response();
    assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn test_app_error_from_act_error_into_response() {
    let act_err = acts::ActError::Action("action failed".to_string());
    let err = AppError::from(act_err);
    let resp = err.into_response();
    assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
}

// ── WebPlugin ──

#[test]
fn test_web_plugin_new() {
    let plugin = WebPlugin::new();
    let _ = plugin;
}

#[test]
fn test_web_plugin_default() {
    let plugin = WebPlugin;
    let _ = plugin;
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Wait until the port is (un)reachable, or give up after 10s.
async fn wait_for_port(port: u16, want_reachable: bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let reachable = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok();
        if reachable == want_reachable {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// `Engine::close` must stop the web server: the transport task selects on the
/// engine shutdown token, so a graceful shutdown releases the listening socket
/// (and stops accepting requests) instead of leaving the process to be killed.
#[tokio::test(flavor = "multi_thread")]
async fn web_server_stops_on_engine_close() {
    let port = free_port();
    let table: toml::Table = toml::from_str(&format!("[web]\nport = {port}\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let engine = acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .start()
        .await
        .unwrap();

    assert!(
        wait_for_port(port, true).await,
        "the web server must accept connections while the engine runs"
    );

    engine.close().await;

    assert!(
        wait_for_port(port, false).await,
        "engine.close() must stop the web server and release its port"
    );
}
