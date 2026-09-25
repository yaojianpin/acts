use crate::{
    DEFAULT_HOST, DEFAULT_QUEUE_SIZE, HttpConfig, WebPlugin,
    objects::{AppError, RespData, RespStatus},
    routes::{PackageParams, pack_list},
};
use axum::{Extension, Json, extract::State, response::IntoResponse};
use serde_json::json;
use std::sync::Arc;
use validator::Validate;

// ── HttpConfig ──

#[test]
fn test_http_config_default() {
    let config = HttpConfig::default();
    assert_eq!(config.port, None);
    assert_eq!(
        config.host, None,
        "an unconfigured transport resolves its host through DEFAULT_HOST"
    );
    assert_eq!(
        config
            .host
            .clone()
            .unwrap_or_else(|| DEFAULT_HOST.to_string()),
        "127.0.0.1",
        "the default bind is loopback, not the wildcard"
    );
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

#[test]
fn test_app_error_bad_request_into_response() {
    let resp = AppError::bad_request("page too deep").into_response();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

/// A request that fails validation is the caller's fault, so it answers `400`
/// — the `500` a server-side failure deserves would tell clients their input
/// was fine and the service was broken.
#[test]
fn test_app_error_from_validation_errors_is_a_400() {
    let params = PackageParams {
        catalog: None,
        search: None,
        order: None,
        size: Some(0),
        page: None,
    };
    let err = AppError::from(params.validate().unwrap_err());
    let resp = err.into_response();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

/// `pack_list` bounds its paging: a page inside the size and offset bounds is
/// served, anything past them is refused with a `400` — including a `page`
/// whose `page * size` overflows `usize`, which wrapped around (debug builds:
/// panicked) before the bound existed, handing back a small offset as if the
/// deep page were page one.
#[tokio::test(flavor = "multi_thread")]
async fn pack_list_serves_pages_inside_the_bounds_and_refuses_the_rest() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());

    let list = |size, page| {
        pack_list(
            state.clone(),
            principal.clone(),
            Json(PackageParams {
                catalog: None,
                search: None,
                order: None,
                size,
                page,
            }),
        )
    };

    // served: a small page, and the widest page at the deepest offset
    for (size, page) in [(Some(10), Some(1)), (Some(100), Some(1001))] {
        let resp = list(size, page)
            .await
            .expect("a page inside the bounds must be served");
        assert_eq!(
            resp.into_response().status(),
            axum::http::StatusCode::OK,
            "size {size:?} page {page:?} must be served"
        );
    }

    // refused: an oversized page, a page past the offset bound, and a page
    // whose product cannot be represented
    for (size, page) in [
        (Some(101), None),
        (Some(100), Some(1002)),
        (Some(100), Some(usize::MAX)),
    ] {
        let err = match list(size, page).await {
            Ok(_) => panic!("a page past the bounds must be refused: size {size:?} page {page:?}"),
            Err(err) => err,
        };
        assert_eq!(
            err.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST,
            "size {size:?} page {page:?} must be a 400"
        );
    }

    engine.close().await;
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

/// Wait until `host:port` is (un)reachable, or give up after 10s.
async fn wait_for_port(host: &str, port: u16, want_reachable: bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let reachable = tokio::net::TcpStream::connect((host, port)).await.is_ok();
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
        wait_for_port("127.0.0.1", port, true).await,
        "the web server must accept connections while the engine runs"
    );

    engine.close().await;

    assert!(
        wait_for_port("127.0.0.1", port, false).await,
        "engine.close() must stop the web server and release its port"
    );
}

/// A bind failure must fail the engine start with its reason, not be logged
/// inside the transport task while the engine reports a clean start: the port
/// here is already taken, so the plugin cannot listen.
#[tokio::test(flavor = "multi_thread")]
async fn bind_conflict_fails_the_engine_start() {
    let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = blocker.local_addr().unwrap().port();

    let table: toml::Table = toml::from_str(&format!("[web]\nport = {port}\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let started = acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .start()
        .await;
    let err = match started {
        Ok(_) => panic!("a taken port must fail the engine start"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains(&port.to_string()),
        "the failure must name the address it could not bind: {err}"
    );
}

/// The default bind is loopback, not the wildcard: after the server is up, a
/// sibling loopback alias of the same port is still free, so a caller on
/// another interface cannot reach the management surface out of the box. The
/// wildcard itself is the wrong probe: Linux refuses a wildcard bind that
/// overlaps a listening specific-address socket regardless of `SO_REUSEADDR`,
/// so it fails with "address already in use" even when the server binds
/// loopback only.
#[tokio::test(flavor = "multi_thread")]
async fn web_server_defaults_to_loopback() {
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

    assert!(wait_for_port("127.0.0.1", port, true).await);

    let alias = std::net::TcpListener::bind(("127.0.0.2", port));
    assert!(
        alias.is_ok(),
        "the web server must hold exactly 127.0.0.1 by default — the loopback \
         alias of the port must stay free and the wildcard must not be held: {alias:?}"
    );
    drop(alias);

    engine.close().await;
}

/// The deployment steers the bind through `[web].host`: the server must hold
/// exactly the configured address, observable as an exact bind conflict on a
/// loopback alias — distinct from the default `127.0.0.1` the other cases use.
#[tokio::test(flavor = "multi_thread")]
async fn web_server_binds_the_configured_host() {
    let port = free_port();
    let table: toml::Table =
        toml::from_str(&format!("[web]\nhost = \"127.0.0.2\"\nport = {port}\n")).unwrap();
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
        wait_for_port("127.0.0.2", port, true).await,
        "the server bound to the configured loopback alias must answer"
    );

    let alias = std::net::TcpListener::bind(("127.0.0.2", port));
    assert!(
        alias.is_err(),
        "the server must bind exactly the configured host: {alias:?}"
    );

    engine.close().await;
}
