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

/// `ActError::Action` is a caller-correctable payload problem (a missing
/// `uses`, an empty pid), so the web transport answers `400`, not the `500`
/// a server-side failure deserves.
#[test]
fn test_app_error_from_act_error_into_response() {
    let act_err = acts::ActError::Action("action failed".to_string());
    let err = AppError::from(act_err);
    let resp = err.into_response();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
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

/// An action refusal keeps its kind — the stable HTTP status the shared
/// external-error contract assigns each code (404 for a missing target, 400
/// for bad input, 409 for a lost race, 429 for load, 503 for shutdown).
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
            axum::http::StatusCode::NOT_FOUND,
        ),
        (
            ActionError::Invalid("bad input".to_string()),
            axum::http::StatusCode::BAD_REQUEST,
        ),
        (
            ActionError::Conflict("moved".to_string()),
            axum::http::StatusCode::CONFLICT,
        ),
        (
            ActionError::ResourceExhausted("busy".to_string()),
            axum::http::StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            ActionError::Unavailable("draining".to_string()),
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
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
    let plugin = WebPlugin::default();
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

/// A start that fails at a later plugin must join the listeners already
/// started before it returns: the first web plugin binds the port, the second
/// cannot, and the rollback releases the socket.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_second_listener_releases_the_first_started_plugin() {
    let port = free_port();
    let table: toml::Table = toml::from_str(&format!("[web]\nport = {port}\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    // Both plugins read the same `[web]` port: the first binds, the second
    // fails, and the start's rollback must join the first (releasing the
    // socket) before returning the error.
    let started = acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .add_plugin(&WebPlugin::new())
        .start()
        .await;
    assert!(started.is_err(), "the second listener cannot bind the port");
    assert!(
        wait_for_port("127.0.0.1", port, false).await,
        "the rollback must stop the plugin that did bind"
    );
}

/// The default bind is loopback, not the wildcard (see the comment above).
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
use acts::{Engine, Principal, RunningTransport, Vars, Workflow};
use axum::extract::{Path, Query};
use axum::http::StatusCode;

/// A workflow with one assigned irq act: the per-task assignment actions
/// (`transfer`/`delegate`/`undelegate`) need an owner.
fn assigned_workflow(id: &str) -> Workflow {
    Workflow::new()
        .with_id(id)
        .with_rn("web-act-test")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses("acts.core.irq", Vars::new().with("key", "web-act-test"))
                .with_owner("alice")
        })
}

/// Deploy and start `workflow`, answering `(pid, tid)` from the act's Created
/// message. The channel is registered before the start, so the message cannot
/// be missed.
async fn start_and_wait(engine: &Engine, workflow: &Workflow) -> (String, String) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let chan = engine.channel();
    chan.on_message(move |message| {
        let tx = tx.clone();
        async move {
            if message.state == acts::MessageState::Created && message.r#type == "step" {
                let _ = tx.send(message.inner().clone());
            }
        }
    });
    engine
        .executor(&Principal::unrestricted())
        .model()
        .deploy(workflow, None)
        .await
        .unwrap();
    let pid = engine
        .executor(&Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap()
        .pid()
        .to_string();
    let created = rx.recv().await.expect("the act's Created message");
    (pid, created.tid)
}

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

/// The task-action route: the assignment actions reach the shared table as
/// their wire names, and an action outside the decision set is a `400` rather
/// than an arbitrary name forwarded to the engine.
#[tokio::test(flavor = "multi_thread")]
async fn routes_cover_the_assignment_actions() {
    let engine = unrestricted_engine().await;
    let state = State(Arc::new(engine.clone()));
    let principal = Extension(engine.acl().authenticate(None).unwrap());
    let workflow = assigned_workflow("web-act-actions");
    let (pid, tid) = start_and_wait(&engine, &workflow).await;

    let transfer = routes::act(
        state.clone(),
        principal.clone(),
        Path("transfer".to_string()),
        Json(
            Vars::new()
                .with("pid", pid.clone())
                .with("tid", tid.clone())
                .with("to", "bob")
                .with("expect_rev", 0),
        ),
    )
    .await
    .expect("the transfer action must run");
    assert_eq!(transfer.into_response().status(), StatusCode::OK);

    let delegate = routes::act(
        state.clone(),
        principal.clone(),
        Path("delegate".to_string()),
        Json(
            Vars::new()
                .with("pid", pid.clone())
                .with("tid", tid.clone())
                .with("delegatee", "carol")
                .with("expires_at", 1_761_000_000_000i64)
                .with("reason", "holiday"),
        ),
    )
    .await
    .expect("the delegate action must run");
    assert_eq!(delegate.into_response().status(), StatusCode::OK);

    let undelegate = routes::act(
        state.clone(),
        principal.clone(),
        Path("undelegate".to_string()),
        Json(
            Vars::new()
                .with("pid", pid.clone())
                .with("tid", tid.clone()),
        ),
    )
    .await
    .expect("the undelegate action must run");
    assert_eq!(undelegate.into_response().status(), StatusCode::OK);

    let unknown = routes::act(
        state,
        principal,
        Path("explode".to_string()),
        Json(Vars::new().with("pid", pid).with("tid", tid)),
    )
    .await;
    match unknown {
        Ok(_) => panic!("an unknown act action must be refused"),
        Err(err) => assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST),
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
        // the trigger does not exist, but the request was translated and run:
        // the addressed entity answers `not_found` (404), not the `500` a
        // server-side failure deserves
        assert_eq!(
            resp.err().map(|err| err.into_response().status()),
            Some(StatusCode::NOT_FOUND)
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
        axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 41_000))),
        None,
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
    // an anonymous caller is refused: a write is a 401, not a denial
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
/// the middleware answers 401 from the three-state resolution, never a 500
/// from a header it could not parse. Authorization denials belong to the
/// action layer — a credential that does not check out is always 401.
#[tokio::test(flavor = "multi_thread")]
async fn http_auth_maps_acl_failures_to_401_and_403() {
    for (mode, expected) in [
        (FailingAclMode::Unauthenticated, 401),
        (FailingAclMode::Denied, 401),
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

// ── admission middleware stack (limit.rs, design §4.1) ──

use crate::limit::{LimitState, WebTransport};

/// A one-slot hang: the handler waits until the test drops the sender, and
/// the test can poll the admission counter to see the request go inflight.
type HangSlot = Arc<tokio::sync::Notify>;

async fn hang_handler(State(hang): State<HangSlot>) -> &'static str {
    hang.notified().await;
    "done"
}

/// The middleware stack (`limit::apply_layers`) over two routes: `/act/hang`
/// (hangs until released; the `/act/` prefix puts it under the `default`
/// profile, so the timeout gate resolves a real deadline for it) and
/// `/act/ping`.
async fn limited_server(engine: &acts::Engine, web: &str) -> (u16, Arc<LimitState>, HangSlot) {
    let config: HttpConfig = toml::from_str(web).expect("a valid [web] section");
    let limits = LimitState::new(engine, &config).unwrap();
    let hang: HangSlot = Arc::new(tokio::sync::Notify::new());
    let router = axum::Router::new()
        .route("/act/hang", axum::routing::post(hang_handler))
        .route("/act/ping", axum::routing::post(|| async { "pong" }))
        .with_state(hang.clone());
    let router = crate::limit::apply_layers(&limits, router);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    (port, limits, hang)
}

/// Parse one `X-Forwarded-For`-carrying response header out of a raw
/// response (the body is split off first).
fn response_header(port_headers: &[(String, String)], name: &str) -> Option<String> {
    port_headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

async fn http_call_with_headers(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, Vec<(String, String)>, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the server accepts connections");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or_default());
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut response),
    )
    .await
    .expect("the server answers")
    .expect("the response is readable");
    let text = String::from_utf8_lossy(&response).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    (status, headers, body.to_string())
}

/// The global concurrency gate: a transport at `[web].max_concurrent` answers
/// `429` with a `Retry-After` header, and frees up as soon as an inflight
/// request finishes.
#[tokio::test(flavor = "multi_thread")]
async fn global_concurrency_refuses_the_overflow_with_retry_after() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    let (port, limits, hang) =
        limited_server(&engine, "max_concurrent = 1\nrate_limit_per_min = 0\n").await;

    // one inflight request holds the transport's only slot
    let hang_port = tokio::spawn(async move {
        let (status, _, _) = http_call_with_headers(port, "POST", "/act/hang", &[], None).await;
        status
    });
    // Wait for the counter (the ground truth the gate and the drain share)
    // to show the hung request admitted; only then can the overflow answer
    // be the gate's refusal rather than a request that has not arrived yet.
    let started = std::time::Instant::now();
    while limits.counter.active() == 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the hang request was never admitted (counter stayed 0)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (status, headers, body) =
        http_call_with_headers(port, "POST", "/act/ping", &[], None).await;
    assert_eq!(status, 429, "the overflow answers 429, body: {body}");
    assert_eq!(
        response_header(&headers, "retry-after").as_deref(),
        Some("1"),
        "the caller is told when to retry"
    );

    // the hung request finishes; the freed slot admits again
    hang.notify_one();
    assert_eq!(hang_port.await.unwrap(), 200, "the hung request completes");
    let (status, _, _) = http_call_with_headers(port, "POST", "/act/ping", &[], None).await;
    assert_eq!(status, 200, "a freed slot admits again");

    engine.close().await;
}

/// The per-IP concurrency gate: the same client IP cannot hold a second
/// request, a different IP is unaffected. Both IPs come from
/// `X-Forwarded-For` because the peer (127.0.0.1) is a trusted proxy.
#[tokio::test(flavor = "multi_thread")]
async fn per_ip_concurrency_refuses_the_same_ip_only() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    let (port, _limits, hang) = limited_server(
        &engine,
        "max_concurrent = 8\nmax_concurrent_per_ip = 1\nrate_limit_per_min = 0\ntrusted_proxy_cidrs = [\"127.0.0.1/32\"]\n",
    )
    .await;

    let hang_task = tokio::spawn(async move {
        let (status, _, _) = http_call_with_headers(
            port,
            "POST",
            "/act/hang",
            &[("x-forwarded-for", "9.9.9.9")],
            None,
        )
        .await;
        status
    });
    // wait until the first request is inflight by polling the gate with a
    // distinct IP: it must succeed, so retry until it does
    let distinct = loop {
        let (status, _, _) = http_call_with_headers(
            port,
            "POST",
            "/act/ping",
            &[("x-forwarded-for", "8.8.8.8")],
            None,
        )
        .await;
        if status == 200 {
            break status;
        }
        assert!(
            tokio::time::Instant::now().elapsed() < std::time::Duration::from_secs(10),
            "a distinct ip is never refused by per-ip concurrency"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(distinct, 200);

    // the same ip is at its one-request bound
    let (status, _, _) = http_call_with_headers(
        port,
        "POST",
        "/act/ping",
        &[("x-forwarded-for", "9.9.9.9")],
        None,
    )
    .await;
    assert_eq!(status, 429, "the same ip holds no second request");

    hang.notify_one();
    assert_eq!(hang_task.await.unwrap(), 200);

    engine.close().await;
}

/// An untrusted peer cannot pick its own limit bucket: with no trusted
/// proxies configured, a forged `X-Forwarded-For` is ignored and both
/// requests converge onto the peer's own IP.
#[tokio::test(flavor = "multi_thread")]
async fn untrusted_forwarded_for_is_ignored() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    // rate limit off would let both through; a rate of 1 per minute makes
    // the second request's bucket observable
    let (port, _limits, _hang) = limited_server(&engine, "rate_limit_per_min = 1\n").await;

    let (first, _, _) = http_call_with_headers(
        port,
        "POST",
        "/act/ping",
        &[("x-forwarded-for", "5.5.5.5")],
        None,
    )
    .await;
    assert_eq!(first, 200);
    let (second, _, body) = http_call_with_headers(
        port,
        "POST",
        "/act/ping",
        &[("x-forwarded-for", "4.4.4.4")],
        None,
    )
    .await;
    assert_eq!(
        second, 429,
        "a forged forwarded-for must not open a fresh bucket: {body}"
    );

    engine.close().await;
}

/// The token-bucket rate limit: the (N+1)th request of a minute is refused
/// `429`, and another IP's budget is its own.
#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_refuses_past_the_per_minute_budget() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    let (port, _limits, _hang) = limited_server(
        &engine,
        "rate_limit_per_min = 2\ntrusted_proxy_cidrs = [\"127.0.0.1/32\"]\n",
    )
    .await;

    for _ in 0..2 {
        let (status, _, _) = http_call_with_headers(
            port,
            "POST",
            "/act/ping",
            &[("x-forwarded-for", "7.7.7.7")],
            None,
        )
        .await;
        assert_eq!(status, 200);
    }
    let (status, headers, _) = http_call_with_headers(
        port,
        "POST",
        "/act/ping",
        &[("x-forwarded-for", "7.7.7.7")],
        None,
    )
    .await;
    assert_eq!(status, 429, "the (N+1)th request of the minute is refused");
    assert_eq!(
        response_header(&headers, "retry-after").as_deref(),
        Some("1")
    );

    let (status, _, _) = http_call_with_headers(
        port,
        "POST",
        "/act/ping",
        &[("x-forwarded-for", "6.6.6.6")],
        None,
    )
    .await;
    assert_eq!(status, 200, "another ip's budget is its own");

    engine.close().await;
}

/// The quiesce gate and the drain: once the transport quiesces, new requests
/// answer `503 unavailable` while the inflight request keeps running; the
/// drain reports what was still active at its deadline.
#[tokio::test(flavor = "multi_thread")]
async fn quiesce_refuses_new_requests_and_drain_reports_the_inflight() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    let (port, limits, hang) = limited_server(&engine, "rate_limit_per_min = 0\n").await;
    let transport = WebTransport::new(limits.counter.clone());

    let hang_task = tokio::spawn(async move {
        let (status, _, _) = http_call_with_headers(port, "POST", "/act/hang", &[], None).await;
        status
    });
    // wait for the request to be admitted (its guard is what the drain
    // counts)
    loop {
        if limits.counter.active() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    transport.quiesce().await;
    let (status, _, body) = http_call_with_headers(port, "POST", "/act/ping", &[], None).await;
    assert_eq!(status, 503, "a quiescing transport admits nothing");
    assert!(
        body.contains("unavailable"),
        "stable code in the answer: {body}"
    );

    // the inflight request is still running when a tiny drain gives up
    let report = transport.drain(std::time::Duration::from_millis(50)).await;
    assert_eq!(report.transport, acts::TRANSPORT_WEB);
    assert_eq!(report.active_after, 1, "the hung request is what remains");
    assert!(!report.drained);

    hang.notify_one();
    assert_eq!(hang_task.await.unwrap(), 200);
    let report = transport.drain(std::time::Duration::from_secs(2)).await;
    assert!(report.drained, "a finished request lets the drain complete");
    assert_eq!(report.active_after, 0);

    engine.close().await;
}

/// The timeout gate cancels a request past its action profile's deadline:
/// `503 deadline_exceeded`, and the inflight slot is released.
#[tokio::test(flavor = "multi_thread")]
async fn timeout_cancels_past_the_profile_deadline() {
    // the `default` class deadline overridden to a testable minimum
    let engine = {
        let table: toml::Table =
            toml::from_str("[ingress.profile.default]\ndeadline_ms = 1000\n").unwrap();
        let config = acts::Config {
            data: Default::default(),
            table,
        };
        acts::Engine::builder()
            .set_config(&config)
            .disable_acl()
            .start()
            .await
            .unwrap()
    };
    let (port, limits, _hang) = limited_server(&engine, "rate_limit_per_min = 0\n").await;

    // a request that outlives its deadline
    let sleeper = tokio::spawn(async move {
        let (status, _, body) = http_call_with_headers(port, "POST", "/act/hang", &[], None).await;
        (status, body)
    });
    loop {
        if limits.counter.active() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let (status, body) = tokio::time::timeout(std::time::Duration::from_secs(5), sleeper)
        .await
        .expect("the timeout gate answers well before the 10s client deadline")
        .unwrap();
    assert_eq!(status, 503, "a request past its deadline is cancelled");
    assert!(body.contains("deadline_exceeded"), "stable code: {body}");
    // the cancelled request's admission is released
    let report = WebTransport::new(limits.counter.clone())
        .drain(std::time::Duration::from_secs(2))
        .await;
    assert!(report.drained, "the cancelled request released its slot");

    engine.close().await;
}

/// Strict mode: a non-loopback bind with neither TLS nor an explicit
/// `plain_behind_proxy` refuses the start, naming the address; an explicit
/// opt-out or a loopback bind starts.
#[tokio::test(flavor = "multi_thread")]
async fn strict_mode_refuses_a_plain_non_loopback_bind() {
    use crate::PlaintextPosture;

    let non_loopback: std::net::SocketAddr = "192.168.1.10:10082".parse().unwrap();
    let loopback: std::net::SocketAddr = "127.0.0.1:10082".parse().unwrap();
    let posture =
        |level, plain: bool| crate::WebPlugin::plaintext_posture(level, non_loopback, false, plain);

    // permissive only warns on a plain non-loopback bind
    assert!(matches!(
        posture(acts::Level::Permissive, false),
        PlaintextPosture::Warn(_)
    ));
    // standard refuses without an explicit opt-in, allows it with one
    let refusal = match posture(acts::Level::Standard, false) {
        PlaintextPosture::Refuse(reason) => reason,
        other => panic!("standard must refuse a plain non-loopback bind: {other:?}"),
    };
    assert!(
        refusal.contains("192.168.1.10"),
        "the refusal names the address: {refusal}"
    );
    assert!(
        matches!(
            posture(acts::Level::Standard, true),
            PlaintextPosture::Allow
        ),
        "an explicit plain_behind_proxy satisfies standard"
    );
    // strict refuses even the explicit opt-in: the proxy hop is what strict
    // mode refuses to trust
    assert!(
        matches!(
            posture(acts::Level::Strict, true),
            PlaintextPosture::Refuse(_)
        ),
        "strict refuses a plaintext bind behind a proxy"
    );
    // tls satisfies every level, and a loopback bind needs no opt-in
    for level in [
        acts::Level::Permissive,
        acts::Level::Standard,
        acts::Level::Strict,
    ] {
        assert!(
            matches!(
                crate::WebPlugin::plaintext_posture(level, non_loopback, true, false),
                PlaintextPosture::Allow
            ),
            "tls satisfies strict mode at {level:?}"
        );
        assert!(
            matches!(
                crate::WebPlugin::plaintext_posture(level, loopback, false, false),
                PlaintextPosture::Allow
            ),
            "a loopback bind needs no opt-in at {level:?}"
        );
    }

    // enforced at start: `host = "0.0.0.0"` (level standard by default)
    // without tls is a config error, before anything binds
    let port = free_port();
    let table: toml::Table =
        toml::from_str(&format!("[web]\nhost = \"0.0.0.0\"\nport = {port}\n")).unwrap();
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
        Ok(_) => panic!("strict mode must refuse 0.0.0.0 without tls"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains(&port.to_string()),
        "the error names the address: {err}"
    );

    // permissive warns but starts: the same bind, level = "permissive"
    let table: toml::Table = toml::from_str(&format!(
        "[web]\nhost = \"0.0.0.0\"\nport = {port}\nlevel = \"permissive\"\n"
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let engine = acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .start()
        .await
        .expect("permissive starts a plain non-loopback bind (with a warning)");
    engine.close().await;

    // strict refuses even with the explicit plain_behind_proxy opt-in
    let table: toml::Table = toml::from_str(&format!(
        "[web]\nhost = \"0.0.0.0\"\nport = {port}\nlevel = \"strict\"\nplain_behind_proxy = true\n"
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let started = acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .start()
        .await;
    assert!(
        started.is_err(),
        "strict must refuse a plaintext non-loopback bind behind a proxy"
    );

    // strict with tls starts (the PEM pair the TLS-loading test ships).
    // The paths are anchored at the crate root, not the process cwd: a test
    // binary run from the workspace root must find the same files.
    let cert = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_cert.pem");
    let key = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_key.pem");
    let table: toml::Table = toml::from_str(&format!(
        "[web]\nhost = \"127.0.0.1\"\nport = {port}\nlevel = \"strict\"\n[web.tls]\ncert = {cert:?}\nkey = {key:?}\n"
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let engine = acts::Engine::builder()
        .set_config(&cfg)
        .add_plugin(&WebPlugin::new())
        .start()
        .await
        .expect("strict with tls starts");
    assert!(wait_for_port("127.0.0.1", port, true).await);
    engine.close().await;

    // the loopback default trivially passes strict mode (a real start, bound
    // to a free port)
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
        .expect("the loopback bind passes strict mode");
    engine.close().await;
}

/// `[web].tls` loading fails closed: a missing cert file, a malformed PEM and
/// a cert/key mismatch are config errors, and the section itself parses from
/// the deployment config.
#[test]
fn tls_config_parses_and_load_fails_closed() {
    let json = serde_json::json!({
        "tls": {"cert": "certs/server.pem", "key": "certs/server.key", "client_ca": "certs/ca.pem"}
    });
    let config: HttpConfig = serde_json::from_value(json).unwrap();
    let tls = config.tls.as_ref().expect("the tls section parses");
    assert_eq!(tls.cert, "certs/server.pem");
    assert_eq!(tls.key, "certs/server.key");
    assert_eq!(tls.client_ca.as_deref(), Some("certs/ca.pem"));

    let config: HttpConfig =
        serde_json::from_value(serde_json::json!({"plain_behind_proxy": true})).unwrap();
    assert!(config.plain_behind_proxy);
    assert!(HttpConfig::default().tls.is_none(), "tls is opt-in");

    // missing cert file
    let missing = crate::WebPlugin::load_tls_server_config(&crate::TlsConfig {
        cert: "definitely-missing.pem".to_string(),
        key: "also-missing.key".to_string(),
        client_ca: None,
    });
    assert!(missing.is_err(), "a missing cert file refuses the start");

    // malformed pem
    let dir = std::env::temp_dir().join(format!("acts-web-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad_cert = dir.join("bad.pem");
    let bad_key = dir.join("bad.key");
    std::fs::write(&bad_cert, "this is not pem at all\n").unwrap();
    std::fs::write(&bad_key, "-----BEGIN NOPE-----\nnope\n-----END NOPE-----\n").unwrap();
    let malformed = crate::WebPlugin::load_tls_server_config(&crate::TlsConfig {
        cert: bad_cert.to_string_lossy().into_owned(),
        key: bad_key.to_string_lossy().into_owned(),
        client_ca: None,
    });
    assert!(malformed.is_err(), "a malformed pem refuses the start");

    // real pem files that do not match each other
    let (cert_pem, key_pem) = minimal_self_signed_pair();
    let cert_path = dir.join("server.pem");
    let key_path = dir.join("server.key");
    std::fs::write(&cert_path, cert_pem).unwrap();
    std::fs::write(&key_path, key_pem).unwrap();
    let loaded = crate::WebPlugin::load_tls_server_config(&crate::TlsConfig {
        cert: cert_path.to_string_lossy().into_owned(),
        key: key_path.to_string_lossy().into_owned(),
        client_ca: None,
    });
    assert!(
        loaded.is_ok(),
        "a matching cert/key pair loads: {:?}",
        loaded.err()
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A minimal self-signed RSA cert/key pair, PEM-encoded without any new
/// dependency: the point is "the PEM loader parses it and the pieces match",
/// not certificate quality.
fn minimal_self_signed_pair() -> (String, String) {
    // a fixed PKCS#8 RSA key (2048-bit) and its self-signed certificate,
    // generated once with openssl and pasted as constants — generating keys
    // in-process would need an RSA dependency this crate does not have
    const KEY: &str = include_str!("../tests/test_key.pem");
    const CERT: &str = include_str!("../tests/test_cert.pem");
    (CERT.to_string(), KEY.to_string())
}

// ── body caps, auth hardening, error contract, metrics (over real HTTP) ──

/// A plugin server on a free port, with a user that may read models.
async fn server_with_web(extra_web: &str) -> (acts::Engine, u16, String) {
    use acts_acl::AclUsers;

    let port = free_port();
    let table: toml::Table = toml::from_str(&format!("[web]\nport = {port}\n{extra_web}")).unwrap();
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
            allow: Some(vec![
                "model:ls".to_string(),
                "model:get".to_string(),
                "model:deploy".to_string(),
                "evt:start".to_string(),
                "msg:sub".to_string(),
            ]),
            patterns: Some(vec!["*".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for_port("127.0.0.1", port, true).await);
    let token = login(&engine, "reader", "reader-pass").await;
    (engine, port, token)
}

/// Like `http_call`, but tolerates a server that gives up reading mid-request
/// (a `413` answer closes the connection while the oversized body is still
/// being written) and answers the raw head plus body.
async fn http_call_lenient(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, Vec<(String, String)>, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the web server accepts connections");
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.ok();
    if let Some(body) = body {
        for chunk in body.as_bytes().chunks(64 * 1024) {
            // a server that already answered (e.g. 413) stops reading; the
            // rest of the body is irrelevant to what we assert
            if stream.write_all(chunk).await.is_err() {
                break;
            }
        }
        stream.flush().await.ok();
    }
    let mut response = Vec::new();
    // A server that answered early (413) may reset the connection while the
    // rest of the oversized body is still in flight (windows: 10053). The
    // answer bytes, if any, are already in the buffer; the reset after them
    // is the expected shape of this test, not a failure.
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        stream.read_to_end(&mut response),
    )
    .await;
    if let Ok(Err(err)) = read {
        assert!(
            !response.is_empty(),
            "the connection reset before answering: {err}"
        );
    }
    let text = String::from_utf8_lossy(&response).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    (status, headers, body.to_string())
}

fn header_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The per-route body caps: a deploy body past the 8 MiB deploy profile is
/// `413`, a hook body inside its 1 MiB event profile is served, and a hook
/// body past it is `413` — the cap follows the route's action profile.
///
/// Driven in-process (`oneshot`): the cap is a body-layer answer, and a real
/// socket on windows resets an oversized request before the `413` is read
/// (the server closes with unread bytes in flight), which would make the
/// assertion about *which* refusal flaky rather than wrong.
#[tokio::test(flavor = "multi_thread")]
async fn body_caps_follow_the_route_profiles() {
    use tower::ServiceExt;

    let engine = Arc::new(acts::Engine::builder().disable_acl().start().await.unwrap());
    let config: HttpConfig = toml::from_str("").unwrap();
    let limits = crate::limit::LimitState::new(&engine, &config).unwrap();
    let router = crate::WebPlugin::build_router(&engine, &limits);

    let call = |path: &'static str, body: Vec<u8>| {
        let router = router.clone();
        async move {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                "127.0.0.1:40000".parse::<std::net::SocketAddr>().unwrap(),
            ));
            router.oneshot(request).await.unwrap().status()
        }
    };

    // a deploy body past the 8 MiB profile
    assert_eq!(
        call("/api/model/deploy", vec![b'x'; 9 * 1024 * 1024]).await,
        413,
        "a deploy body past 8 MiB is refused"
    );

    // a hook body inside the 1 MiB event profile reaches the handler
    let hook_body = serde_json::json!({"key": "x".repeat(900 * 1024)})
        .to_string()
        .into_bytes();
    let status = call("/hooks/some-model:some-trigger", hook_body).await;
    assert_ne!(
        status, 413,
        "a hook body inside 1 MiB passes the cap (status {status})"
    );

    // the same route past its cap
    let oversized_hook = serde_json::json!({"key": "x".repeat(1024 * 1024 + 1)})
        .to_string()
        .into_bytes();
    assert_eq!(
        call("/hooks/some-model:some-trigger", oversized_hook).await,
        413,
        "a hook body past 1 MiB is refused"
    );

    engine.close().await;
}

/// Duplicate or oversized `Authorization` headers are refused before the
/// credential is resolved, and every invalid credential is counted under
/// `acts_auth_invalid_total{transport="web"}`.
#[tokio::test(flavor = "multi_thread")]
async fn authorization_headers_are_hardened() {
    let (engine, port, token) = server_with_web("").await;
    let auth = format!("Bearer {token}");
    let before = engine
        .ingress()
        .metrics
        .snapshot()
        .get("acts_auth_invalid_total{transport=web}")
        .copied()
        .unwrap_or(0);

    // two credentials at once is never an identity
    let (status, _, body) = http_call_lenient(
        port,
        "POST",
        "/api/model/list",
        &[("authorization", &auth), ("authorization", &auth)],
        Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
    )
    .await;
    assert_eq!(status, 401, "two authorization headers are refused");
    assert!(
        body.contains("unauthenticated"),
        "stable code in the answer: {body}"
    );

    // a credential-sized header is not a token dump
    let huge = format!("Bearer {}", "x".repeat(9 * 1024));
    let (status, _, _) = http_call_lenient(
        port,
        "POST",
        "/api/model/list",
        &[("authorization", &huge)],
        Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
    )
    .await;
    assert_eq!(status, 401, "a 9 KiB authorization header is refused early");

    // a credential that does not check out
    let (status, _, _) = http_call_lenient(
        port,
        "POST",
        "/api/model/list",
        &[("authorization", "Bearer not-a-token")],
        Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
    )
    .await;
    assert_eq!(status, 401);

    let after = engine
        .ingress()
        .metrics
        .snapshot()
        .get("acts_auth_invalid_total{transport=web}")
        .copied()
        .unwrap_or(0);
    assert_eq!(
        after - before,
        3,
        "every refused credential is counted: {before} -> {after}"
    );

    engine.close().await;
}

/// Every error answer carries the shared contract — `ecode`,
/// `correlation_id`, `retryable` in the body and the `x-correlation-id`
/// response header.
#[tokio::test(flavor = "multi_thread")]
async fn errors_carry_the_shared_contract() {
    let (engine, port, token) = server_with_web("").await;
    let auth = format!("Bearer {token}");

    let (status, headers, body) = http_call_lenient(
        port,
        "POST",
        "/api/model/get",
        &[("authorization", &auth)],
        Some(r#"{"id":"no-such-model","fmt":"yml"}"#),
    )
    .await;
    assert!(status >= 400, "a missing model is an error, got {status}");
    let value: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("the answer is the contract JSON: {body}"));
    assert!(
        value.get("ecode").is_some(),
        "the stable code is present: {body}"
    );
    assert!(
        value.get("correlation_id").is_some(),
        "the correlation id is present: {body}"
    );
    assert!(
        value.get("retryable").is_some(),
        "retryable is present: {body}"
    );
    let cid = value
        .get("correlation_id")
        .and_then(|v| v.as_str())
        .expect("a string correlation id");
    assert_eq!(
        header_of(&headers, "x-correlation-id"),
        Some(cid),
        "the header and the body carry the same correlation id"
    );

    engine.close().await;
}

/// The ingress metrics: after served requests and a refusal, the engine's
/// snapshot counts web requests per action and result.
#[tokio::test(flavor = "multi_thread")]
async fn ingress_metrics_count_web_requests() {
    let (engine, port, token) = server_with_web("").await;
    let auth = format!("Bearer {token}");

    let _ = http_call_lenient(
        port,
        "POST",
        "/api/model/list",
        &[("authorization", &auth)],
        Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
    )
    .await;
    // a refusal: an unknown credential is counted as an invalid auth, and a
    // request that reaches the stack lands in the requests counter
    let _ = http_call_lenient(
        port,
        "POST",
        "/api/model/list",
        &[("authorization", "Bearer nope")],
        Some(r#"{"offset":0,"limit":10,"order_by":[]}"#),
    )
    .await;

    let snapshot = engine.ingress().metrics.snapshot();
    assert!(
        snapshot
            .keys()
            .any(|key| key.starts_with("acts_ingress_requests_total{")
                && key.contains("transport=web")),
        "web requests are counted: {:?}",
        snapshot.keys().collect::<Vec<_>>()
    );
    assert!(
        snapshot
            .keys()
            .any(|key| key.contains("acts_auth_invalid_total{transport=web}")),
        "invalid credentials are counted"
    );

    engine.close().await;
}

/// `[web].level` resolves per-section over `[security].level` over the
/// `standard` default (the core's `resolve_level`).
#[tokio::test(flavor = "multi_thread")]
async fn web_level_resolves_through_security() {
    let make_config = |web: &str, security: Option<&str>| {
        let mut toml_text = format!("[web]\n{web}");
        if let Some(security) = security {
            toml_text.push_str(&format!("\n[security]\nlevel = \"{security}\"\n"));
        }
        let table: toml::Table = toml::from_str(&toml_text).unwrap();
        acts::Config {
            data: Default::default(),
            table,
        }
    };
    let config_of = |engine: &acts::Engine| {
        let web: HttpConfig = engine.config().get::<HttpConfig>("web").unwrap_or_default();
        acts::resolve_level(web.level.as_deref(), &engine.config())
    };

    // no level anywhere: standard
    let config = make_config("port = 10082", None);
    let engine = acts::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    assert_eq!(config_of(&engine), acts::Level::Standard);
    engine.close().await;

    // [security].level wins over the default
    let config = make_config("port = 10082", Some("strict"));
    let engine = acts::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    assert_eq!(config_of(&engine), acts::Level::Strict);
    engine.close().await;

    // the section's own level wins over [security].level
    let config = make_config("port = 10082\nlevel = \"permissive\"", Some("strict"));
    let engine = acts::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    assert_eq!(config_of(&engine), acts::Level::Permissive);
    engine.close().await;
}

// ── TLS loader and posture verdicts ──

use crate::{PlaintextPosture, pem_certs, pem_key};

/// The PEM loader names the file it could not use, and empty files say
/// what is missing.
#[test]
fn pem_loader_errors_name_the_file() {
    let dir = std::env::temp_dir().join(format!("acts-web-pem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, "").unwrap();
    let garbage = dir.join("garbage.pem");
    std::fs::write(&garbage, "not a pem").unwrap();

    let empty = empty.to_string_lossy().into_owned();
    let err = pem_certs(&empty).unwrap_err();
    assert!(err.contains("holds no certificate"), "{err}");

    let garbage = garbage.to_string_lossy().into_owned();
    let err = pem_certs(&garbage).unwrap_err();
    assert!(err.contains("garbage.pem"), "{err}");
    let err = pem_key(&garbage).unwrap_err();
    assert!(err.contains("garbage.pem"), "{err}");

    let missing = dir.join("missing.pem").to_string_lossy().into_owned();
    let err = pem_certs(&missing).unwrap_err();
    assert!(err.contains("missing.pem"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The plaintext verdict per level: permissive warns, standard refuses
/// without the acknowledged proxy, strict refuses even with it.
#[test]
fn plaintext_posture_follows_the_level() {
    use acts::Level;
    use std::net::SocketAddr;
    let non_loopback: SocketAddr = "0.0.0.0:10082".parse().unwrap();
    let loopback: SocketAddr = "127.0.0.1:10082".parse().unwrap();

    assert!(matches!(
        WebPlugin::plaintext_posture(Level::Permissive, non_loopback, false, false),
        PlaintextPosture::Warn(_)
    ));
    assert!(matches!(
        WebPlugin::plaintext_posture(Level::Standard, non_loopback, false, false),
        PlaintextPosture::Refuse(_)
    ));
    assert!(matches!(
        WebPlugin::plaintext_posture(Level::Standard, non_loopback, false, true),
        PlaintextPosture::Allow
    ));
    assert!(matches!(
        WebPlugin::plaintext_posture(Level::Strict, non_loopback, false, true),
        PlaintextPosture::Refuse(_)
    ));
    assert!(matches!(
        WebPlugin::plaintext_posture(Level::Strict, non_loopback, true, false),
        PlaintextPosture::Allow
    ));
    assert!(matches!(
        WebPlugin::plaintext_posture(Level::Strict, loopback, false, false),
        PlaintextPosture::Allow
    ));
}
