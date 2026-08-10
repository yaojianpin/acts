use crate::{
    HttpConfig, WebPlugin,
    objects::{AppError, RespData, RespStatus},
};
use axum::response::IntoResponse;
use serde_json::json;

// ── HttpConfig ──

#[test]
fn test_http_config_default() {
    let config = HttpConfig::default();
    assert_eq!(config.port, None);
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
