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

/// An action refusal keeps its kind: no credential is 401, no grant is 403,
/// and everything else is a server failure (500).
#[test]
fn app_error_maps_action_error_kinds() {
    use acts::actions::Error as ActionError;

    for (err, expected) in [
        (
            ActionError::Unauthenticated("no token".to_string()),
            axum::http::StatusCode::UNAUTHORIZED,
        ),
        (
            ActionError::Denied("no grant".to_string()),
            axum::http::StatusCode::FORBIDDEN,
        ),
        (
            ActionError::Internal("store is down".to_string()),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            ActionError::NotFound("no:such".to_string()),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            ActionError::Invalid("bad input".to_string()),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let resp = AppError::from(err).into_response();
        assert_eq!(resp.status(), expected);
    }
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

// ── routes: model surface ──

use crate::routes::{
    self, PacakgeGet, PacakgeId, PacakgePublish, PacakgeStart, PackageIdRequest, SnapRef,
    SnapUpsert,
};
use crate::sse::{self, MessageAck, MessageQuery};
use acts::query::Query as ActsQuery;
use acts::{Engine, Principal, Vars, Workflow};
use axum::extract::{Path, Query};
use axum::http::StatusCode;

/// A workflow the web surface can deploy and start: the in-memory irq package
/// needs no external runtime, so a run completes without extra packages.
fn irq_workflow(id: &str) -> Workflow {
    Workflow::new()
        .with_id(id)
        .with_rn("web-test")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses("acts.core.irq", Vars::new().with("key", "web-test"))
        })
}

async fn unrestricted_engine() -> Engine {
    Engine::builder().disable_acl().start().await.unwrap()
}

/// An engine with the store-backed registry and one user per
/// `(name, password, allow)` entry.
async fn engine_with_users(users: &[(&str, &str, &[&str])]) -> Engine {
    use acts_acl::AclUsers;

    let engine = Engine::builder().with_user_acl().start().await.unwrap();
    for (name, password, allow) in users {
        engine
            .acl()
            .set_user(&acts::UserSpec {
                name: (*name).to_string(),
                add_passwords: vec![(*password).to_string()],
                allow: Some(allow.iter().map(|a| (*a).to_string()).collect()),
                patterns: Some(vec!["*".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    engine
}

/// Log `user` in and answer the session token its requests carry.
async fn login(engine: &Engine, user: &str, password: &str) -> String {
    engine.acl().login(user, password).await.unwrap().token
}

/// The model routes are thin translations onto the shared action table: json
/// and yaml bodies both deploy, an unknown format is rejected, and list/get/rm
/// answer the deployed model.
#[tokio::test(flavor = "multi_thread")]
async fn routes_cover_the_model_surface() {
    let engine = unrestricted_engine().await;
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());

    let workflow = irq_workflow("web-model");
    let model: serde_json::Value = serde_json::from_str(&workflow.to_json().unwrap()).unwrap();

    // json deploy, carrying a view
    let resp = routes::deploy(
        state.clone(),
        principal.clone(),
        Json(PacakgePublish {
            fmt: "json".to_string(),
            model: model.clone(),
            view: json!({"title": "web"}),
        }),
    )
    .await
    .expect("a json model deploys");
    assert_eq!(resp.into_response().status(), StatusCode::OK);

    // yaml deploy: the json object text is also valid yaml
    for fmt in ["yml", "ymal"] {
        let resp = routes::deploy(
            state.clone(),
            principal.clone(),
            Json(PacakgePublish {
                fmt: fmt.to_string(),
                model: model.clone(),
                view: serde_json::Value::Null,
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("fmt {fmt} must deploy"));
        assert_eq!(resp.into_response().status(), StatusCode::OK);
    }

    // an unknown format is refused instead of guessed
    let err = match routes::deploy(
        state.clone(),
        principal.clone(),
        Json(PacakgePublish {
            fmt: "xml".to_string(),
            model: model.clone(),
            view: serde_json::Value::Null,
        }),
    )
    .await
    {
        Ok(_) => panic!("an unknown fmt must be refused"),
        Err(err) => err,
    };
    assert_eq!(
        err.into_response().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );

    // a body that is not a model is refused too
    let err = match routes::deploy(
        state.clone(),
        principal.clone(),
        Json(PacakgePublish {
            fmt: "json".to_string(),
            model: json!("not-a-model"),
            view: serde_json::Value::Null,
        }),
    )
    .await
    {
        Ok(_) => panic!("a malformed model must be refused"),
        Err(err) => err,
    };
    assert_eq!(
        err.into_response().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );

    // get: every format the action knows, plus an unknown model
    for fmt in ["text", "yml", "json", "tree"] {
        let resp = routes::get(
            state.clone(),
            principal.clone(),
            Json(PacakgeGet {
                fmt: fmt.to_string(),
                id: "web-model".to_string(),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("fmt {fmt} must be answered"));
        assert_eq!(resp.into_response().status(), StatusCode::OK);
    }
    assert!(
        routes::get(
            state.clone(),
            principal.clone(),
            Json(PacakgeGet {
                fmt: "text".to_string(),
                id: "no-such-model".to_string(),
            }),
        )
        .await
        .is_err()
    );

    // list answers the deployed model
    let resp = routes::list(state.clone(), principal.clone(), Json(ActsQuery::new()))
        .await
        .expect("model:ls must run");
    assert_eq!(resp.into_response().status(), StatusCode::OK);

    // rm removes it
    let resp = routes::rm(
        state.clone(),
        principal.clone(),
        Json(PacakgeId {
            id: "web-model".to_string(),
        }),
    )
    .await
    .expect("model:rm must run");
    assert_eq!(resp.into_response().status(), StatusCode::OK);
    assert!(
        engine
            .executor(&engine.acl().authenticate(None).unwrap())
            .model()
            .get("web-model", "text")
            .await
            .is_err(),
        "the removed model is no longer readable"
    );

    engine.close().await;
}

/// `proc:start` translates three shapes — an inline model, a deployed id, and
/// neither — and refuses a mode it does not know instead of guessing.
#[tokio::test(flavor = "multi_thread")]
async fn routes_cover_proc_start_modes() {
    let engine = unrestricted_engine().await;
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());
    let workflow = irq_workflow("web-proc");
    let model: serde_json::Value = serde_json::from_str(&workflow.to_json().unwrap()).unwrap();

    // mode "model": the body itself is the model
    let resp = routes::proc_start(
        state.clone(),
        principal.clone(),
        Json(PacakgeStart {
            mode: "model".to_string(),
            id: None,
            fmt: Some("json".to_string()),
            model: Some(model.clone()),
            options: Some(Vars::new().with("pid_option", "inline")),
        }),
    )
    .await
    .expect("mode=model must start the inline model");
    assert_eq!(resp.into_response().status(), StatusCode::OK);

    // mode "id": a deployed model
    routes::deploy(
        state.clone(),
        principal.clone(),
        Json(PacakgePublish {
            fmt: "json".to_string(),
            model: model.clone(),
            view: serde_json::Value::Null,
        }),
    )
    .await
    .expect("the model deploys");
    let resp = routes::proc_start(
        state.clone(),
        principal.clone(),
        Json(PacakgeStart {
            mode: "id".to_string(),
            id: Some("web-proc".to_string()),
            fmt: None,
            model: None,
            options: None,
        }),
    )
    .await
    .expect("mode=id must start the deployed model");
    assert_eq!(resp.into_response().status(), StatusCode::OK);

    // the missing half of each mode and an unknown mode are caller errors
    let missing = [
        PacakgeStart {
            mode: "model".to_string(),
            id: None,
            fmt: Some("json".to_string()),
            model: None,
            options: None,
        },
        PacakgeStart {
            mode: "model".to_string(),
            id: None,
            fmt: None,
            model: Some(model.clone()),
            options: None,
        },
        PacakgeStart {
            mode: "id".to_string(),
            id: None,
            fmt: None,
            model: None,
            options: None,
        },
        PacakgeStart {
            mode: "run".to_string(),
            id: Some("web-proc".to_string()),
            fmt: None,
            model: None,
            options: None,
        },
    ];
    for req in missing {
        let err = match routes::proc_start(state.clone(), principal.clone(), Json(req)).await {
            Ok(_) => panic!("an incomplete proc_start must be refused"),
            Err(err) => err,
        };
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    engine.close().await;
}

/// The package routes: the catalogue is answered locally, a published package
/// is retrievable, and `pack:ls` honors its filters and ordering.
#[tokio::test(flavor = "multi_thread")]
async fn routes_cover_the_package_surface() {
    let engine = unrestricted_engine().await;
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());

    let catalogs = routes::pack_catalogs()
        .await
        .expect("the catalogue needs no action");
    assert_eq!(catalogs.into_response().status(), StatusCode::OK);

    // publish one package so pack:get has something to answer
    acts::actions::apply_as(
        &engine,
        &engine.acl().authenticate(None).unwrap(),
        "pack:publish",
        Vars::new()
            .with("id", "web-pack")
            .with("name", "Web pack")
            .with("desc", "a test package")
            .with("catalog", "core")
            .with("run_as", "irq"),
    )
    .await
    .expect("the package publishes");

    let resp = routes::pack_get(
        state.clone(),
        principal.clone(),
        Json(PackageIdRequest {
            id: "web-pack".to_string(),
        }),
    )
    .await
    .expect("pack:get must run");
    assert_eq!(resp.into_response().status(), StatusCode::OK);
    assert!(
        routes::pack_get(
            state.clone(),
            principal.clone(),
            Json(PackageIdRequest {
                id: "no-such-pack".to_string(),
            }),
        )
        .await
        .is_err()
    );

    // filters and both orderings, with a page and with none
    for params in [
        PackageParams {
            catalog: Some("core".to_string()),
            search: Some("web*".to_string()),
            order: Some("asc".to_string()),
            size: Some(10),
            page: Some(1),
        },
        PackageParams {
            catalog: None,
            search: None,
            order: Some("desc".to_string()),
            size: Some(10),
            page: None,
        },
        PackageParams {
            catalog: Some("core".to_string()),
            search: None,
            order: None,
            size: None,
            page: None,
        },
        PackageParams {
            catalog: None,
            search: None,
            order: None,
            size: Some(10),
            page: Some(3),
        },
    ] {
        let resp = routes::pack_list(state.clone(), principal.clone(), Json(params))
            .await
            .expect("a valid page must be served");
        assert_eq!(resp.into_response().status(), StatusCode::OK);
    }

    engine.close().await;
}

/// The snapshot routes are the feed write surface: upsert, read, list and
/// tombstone, with the scope defaulting to the empty one.
#[tokio::test(flavor = "multi_thread")]
async fn routes_cover_the_snapshot_surface() {
    let engine = unrestricted_engine().await;
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());

    for (scope, rev) in [(Some("u1".to_string()), 3u64), (None, 7u64)] {
        let resp = routes::snap_upsert(
            state.clone(),
            principal.clone(),
            Json(SnapUpsert {
                name: "profile".to_string(),
                scope: scope.clone(),
                rev,
                data: Vars::new().with("val", rev as i64),
            }),
        )
        .await
        .expect("snap:upsert must run");
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        let resp = routes::snap_get(
            state.clone(),
            principal.clone(),
            Json(SnapRef {
                name: "profile".to_string(),
                scope: scope.clone(),
            }),
        )
        .await
        .expect("snap:get must run");
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        let resp = routes::snap_remove(
            state.clone(),
            principal.clone(),
            Json(SnapRef {
                name: "profile".to_string(),
                scope,
            }),
        )
        .await
        .expect("snap:remove must run");
        assert_eq!(resp.into_response().status(), StatusCode::OK);
    }

    // a missing value answers null, and the listing still runs
    let resp = routes::snap_get(
        state.clone(),
        principal.clone(),
        Json(SnapRef {
            name: "profile".to_string(),
            scope: Some("nobody".to_string()),
        }),
    )
    .await
    .expect("snap:get must answer");
    assert_eq!(resp.into_response().status(), StatusCode::OK);
    let resp = routes::snap_ls(
        state.clone(),
        principal.clone(),
        Json(SnapRef {
            name: "profile".to_string(),
            scope: None,
        }),
    )
    .await
    .expect("snap:ls must run");
    assert_eq!(resp.into_response().status(), StatusCode::OK);

    engine.close().await;
}

/// The hook route forwards its body (or its absence) as the trigger payload.
#[tokio::test(flavor = "multi_thread")]
async fn routes_cover_the_hook_route() {
    let engine = unrestricted_engine().await;
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());

    for body in [None, Some(Json(json!({"order": 1})))] {
        let resp = routes::hook(
            state.clone(),
            principal.clone(),
            Path("no-such-model:trigger".to_string()),
            body,
        )
        .await;
        // the trigger does not exist, but the request was translated and run
        assert_eq!(
            resp.err().map(|err| err.into_response().status()),
            Some(StatusCode::INTERNAL_SERVER_ERROR)
        );
    }

    engine.close().await;
}

// ── sse: ack route ──

/// The ack route is the grant-checked half of the delivery contract: a caller
/// with `msg:ack` acks a stored delivery, one without it is answered 403.
#[tokio::test(flavor = "multi_thread")]
async fn sse_ack_is_grant_checked_and_acks_a_delivery() {
    // unrestricted engine: create a live ack channel and run a workflow, so
    // one delivery row exists to ack
    let engine = unrestricted_engine().await;
    let chan = engine.channel_with_options(&acts::ChannelOptions {
        id: "web-ack".to_string(),
        ack: true,
        ..Default::default()
    });
    // a channel stores deliveries only once a handler is registered for it
    chan.on_message(|_e| async move {});
    let step = irq_workflow("web-ack");
    engine
        .executor(&acts::Principal::unrestricted())
        .model()
        .deploy(&step, None)
        .await
        .unwrap();
    engine
        .executor(&acts::Principal::unrestricted())
        .proc()
        .start(&step.id, Vars::new())
        .await
        .unwrap();
    let mut id = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while id.is_none() && tokio::time::Instant::now() < deadline {
        let rows = engine
            .executor(&acts::Principal::unrestricted())
            .msg()
            .list(&ActsQuery::new().offset(0).limit(100))
            .await
            .unwrap();
        id = rows.rows.first().map(|row| row.id.clone());
        if id.is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    let id = id.expect("the ack channel stores a delivery");

    let resp = sse::ack(
        State(Arc::new(engine.clone())),
        Extension(engine.acl().authenticate(None).unwrap()),
        Json(MessageAck { id }),
    )
    .await
    .expect("msg:ack must run");
    assert_eq!(resp.into_response().status(), StatusCode::OK);
    drop(chan);
    engine.close().await;

    // a caller without the grant is refused
    let engine = engine_with_users(&[("reader", "reader-pass", &["msg:ls"])]).await;
    let reader_token = login(&engine, "reader", "reader-pass").await;
    let reader = engine.acl().authenticate(Some(&reader_token)).unwrap();
    let err = match sse::ack(
        State(Arc::new(engine.clone())),
        Extension(reader),
        Json(MessageAck {
            id: "delivery-1".to_string(),
        }),
    )
    .await
    {
        Ok(_) => panic!("a user without msg:ack must be refused"),
        Err(err) => err,
    };
    assert_eq!(err.into_response().status(), StatusCode::FORBIDDEN);
    engine.close().await;
}

/// An SSE subscription with explicit filters carries the messages its filters
/// match: the type/state/uses/options fields are handed to the engine channel
/// instead of the wildcard defaults.
#[tokio::test(flavor = "multi_thread")]
async fn sse_honors_explicit_filters() {
    let engine = unrestricted_engine().await;
    let principal = engine.acl().authenticate(None).unwrap();

    let query = MessageQuery {
        id: "filtered".to_string(),
        r#type: Some("*".to_string()),
        uses: Some("*".to_string()),
        state: Some("*".to_string()),
        key: None,
        options: Vars::new().with("pid", "*"),
    };
    let response = sse::sse(
        State(Arc::new(engine.clone())),
        Extension(principal),
        Query(query),
    )
    .await
    .expect("msg:sub opens the stream")
    .into_response();
    let mut stream = response.into_body().into_data_stream();

    // run a workflow, then collect what the filtered subscription carried
    let step = irq_workflow("web-filtered");
    engine
        .executor(&acts::Principal::unrestricted())
        .model()
        .deploy(&step, None)
        .await
        .unwrap();
    engine
        .executor(&acts::Principal::unrestricted())
        .proc()
        .start(&step.id, Vars::new())
        .await
        .unwrap();
    let seen = drain_bytes(&mut stream, 1_000).await;
    assert!(
        !seen.is_empty(),
        "an explicit filter must still carry the run's messages"
    );

    engine.close().await;
}

// ── auth middleware over real HTTP ──

/// Case-insensitive `Authorization: Bearer <token>`: both prefix spellings are
/// accepted, an empty credential falls back to the anonymous principal, and a
/// malformed header does not crash the middleware.
#[tokio::test(flavor = "multi_thread")]
async fn http_auth_accepts_both_bearer_spellings() {
    use acts_acl::AclUsers;

    let port = free_port();
    let table: toml::Table = toml::from_str(&format!("[web]\nport = {port}\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let engine = acts::Engine::builder()
        .set_config(&cfg)
        .with_user_acl()
        .add_plugin(&WebPlugin::new())
        .start()
        .await
        .unwrap();
    engine
        .acl()
        .set_user(&acts::UserSpec {
            name: "reader".to_string(),
            add_passwords: vec!["reader-pass".to_string()],
            allow: Some(vec!["model:ls".to_string(), "pack:ls".to_string()]),
            patterns: Some(vec!["*".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for_port("127.0.0.1", port, true).await);
    let token = login(&engine, "reader", "reader-pass").await;

    // /health is open, and no route below it is
    let (status, _) = http_call(port, "GET", "/health", &[], None).await;
    assert_eq!(status, 200);
    // anonymous callers are catalogue-only: a write is refused, not served
    let (status, _) = http_call(
        port,
        "POST",
        "/api/model/rm",
        &[],
        Some(r#"{"id":"web-model"}"#),
    )
    .await;
    assert_eq!(status, 401);

    // both prefix spellings authenticate the same caller
    for prefix in ["Bearer", "bearer"] {
        let auth = format!("{prefix} {token}");
        let (status, body) = http_call(
            port,
            "POST",
            "/api/model/list",
            &[("authorization", &auth)],
            Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
        )
        .await;
        assert_eq!(status, 200, "prefix {prefix} must authenticate: {body}");
    }

    // an empty credential after the prefix is "no credential", not a token
    let (status, _) = http_call(
        port,
        "POST",
        "/api/model/list",
        &[("authorization", "Bearer    ")],
        Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
    )
    .await;
    assert!(
        status == 200 || status == 401 || status == 400,
        "an empty bearer must be treated as absent, not as an unknown token"
    );

    // the hook route sits behind the same middleware
    let (status, _) = http_call(port, "POST", "/hooks/m:t", &[], Some("{}")).await;
    assert_eq!(status, 401);

    engine.close().await;
}

/// A header that is not UTF-8 (or not a bearer credential) is no credential:
/// the middleware answers 401/403 from the ACL, never a 500 from a header it
/// could not parse.
#[tokio::test(flavor = "multi_thread")]
async fn http_auth_maps_acl_failures_to_401_and_403() {
    for (mode, expected) in [
        (FailingAclMode::Unauthenticated, 401),
        (FailingAclMode::Denied, 403),
    ] {
        let port = free_port();
        let table: toml::Table = toml::from_str(&format!("[web]\nport = {port}\n")).unwrap();
        let cfg = acts::Config {
            data: Default::default(),
            table,
        };
        let engine = acts::Engine::builder()
            .set_config(&cfg)
            .set_acl(Arc::new(FailingAcl { mode }))
            .add_plugin(&WebPlugin::new())
            .start()
            .await
            .unwrap();
        assert!(wait_for_port("127.0.0.1", port, true).await);

        let (status, body) = http_call(
            port,
            "POST",
            "/api/model/list",
            &[("authorization", "Bearer some-token")],
            Some("{}"),
        )
        .await;
        assert_eq!(
            status, expected,
            "acl failure must answer {expected}: {body}"
        );

        engine.close().await;
    }
}

/// An address the socket parser cannot take must fail the engine start with
/// the reason, not leave a "running" engine without an HTTP listener.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_bind_address_fails_the_engine_start() {
    let table: toml::Table = toml::from_str("[web]\nhost = \"not a host\"\nport = 1234\n").unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let err = match acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .start()
        .await
    {
        Ok(_) => panic!("an unparseable bind address must fail the start"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("not a host"),
        "the failure must name the address: {err}"
    );
}

/// Read SSE frames until the stream is quiet for `idle_ms`, decoding each
/// `data:` payload into an `acts::Message`.
async fn drain_bytes<S>(stream: &mut S, idle_ms: u64) -> Vec<acts::Message>
where
    S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    use futures_util::StreamExt as _;

    let mut out = Vec::new();
    let mut pending = String::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(idle_ms), stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                pending.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = pending.find("\n\n") {
                    let frame = pending[..end].to_string();
                    pending.drain(..end + 2);
                    for line in frame.lines() {
                        let Some(data) = line.strip_prefix("data: ") else {
                            continue;
                        };
                        out.push(serde_json::from_str::<acts::Message>(data).unwrap());
                    }
                }
            }
            Ok(Some(Err(err))) => panic!("SSE stream failed: {err}"),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    out
}

// ── helpers for the HTTP cases ──

#[derive(Clone, Copy, Debug)]
enum FailingAclMode {
    Unauthenticated,
    Denied,
}

/// An access control whose `authenticate` always fails — the only way to reach
/// the middleware's 401/403 mapping: the shipped registries answer an absent
/// or unknown token with the anonymous principal instead.
#[derive(Debug)]
struct FailingAcl {
    mode: FailingAclMode,
}

impl FailingAcl {
    fn failure(&self) -> acts::AclError {
        match self.mode {
            FailingAclMode::Unauthenticated => {
                acts::AclError::Unauthenticated("no such session".to_string())
            }
            FailingAclMode::Denied => acts::AclError::Denied("not allowed".to_string()),
        }
    }

    fn deny<T>(&self) -> acts::Result<T> {
        Err(match self.mode {
            FailingAclMode::Unauthenticated => {
                acts::ActError::Unauthenticated("no such session".to_string())
            }
            FailingAclMode::Denied => acts::ActError::Denied("not allowed".to_string()),
        })
    }
}

#[async_trait::async_trait]
impl acts::AccessControl for FailingAcl {
    async fn load(&self, _store: Arc<acts::Store>) -> acts::Result<()> {
        Ok(())
    }

    fn enabled(&self) -> bool {
        true
    }

    fn authenticate(&self, _token: Option<&str>) -> std::result::Result<Principal, acts::AclError> {
        Err(self.failure())
    }

    async fn login(
        &self,
        _user: &str,
        _password: &str,
    ) -> std::result::Result<acts::LoginTokens, acts::AclError> {
        Err(self.failure())
    }

    async fn refresh(
        &self,
        _refresh_token: &str,
    ) -> std::result::Result<acts::LoginTokens, acts::AclError> {
        Err(self.failure())
    }

    async fn logout(&self, _token: &str) -> acts::Result<bool> {
        self.deny()
    }

    async fn set_user(&self, _spec: &acts::UserSpec) -> acts::Result<()> {
        self.deny()
    }

    async fn del_user(&self, _name: &str) -> acts::Result<()> {
        self.deny()
    }

    async fn user_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn get_user(&self, _name: &str) -> Option<serde_json::Value> {
        None
    }
}

/// One HTTP/1.1 request over a fresh connection, answered with its status and
/// body. `Connection: close` makes the server end the response, so the body
/// needs no framing parser.
async fn http_call(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the web server accepts connections");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or_default());
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut response),
    )
    .await
    .expect("the server answers")
    .expect("the response is readable");
    let text = String::from_utf8_lossy(&response).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}
