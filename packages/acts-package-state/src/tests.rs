//! Tests for the state package: the config it reads, the errors it reports,
//! and the GET/SET round trip over a Redis-speaking server.
//!
//! The round-trip tests do not need a real Redis installation: a minimal
//! RESP server is started on an ephemeral loopback port, which is what the
//! package's client talks to. The protocol surface the client uses is tiny
//! (`PING`, `GET`, `SET` plus the connection's ignored `CLIENT SETINFO`), and
//! answering it here keeps the tests hermetic while still exercising the real
//! `redis` client and its connection code.

use crate::StatePackage;
use acts::{ActPackage, Config, Engine, Principal, Step, Vars, Workflow};
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn config(toml_text: &str) -> Config {
    Config {
        data: Default::default(),
        table: toml::from_str::<toml::Table>(toml_text).unwrap(),
    }
}

fn config_with_uri(uri: &str) -> Config {
    config(&format!("[state]\ndatabase_uri = \"{uri}\"\n"))
}

// ---- the package definition and configuration ----

#[test]
fn definition_describes_get_and_set() {
    let meta = StatePackage::definition();
    assert_eq!(meta.id, "acts.app.state");
    assert_eq!(meta.version, "0.1.0");
    assert!(meta.schema["properties"]["op"]["enum"].is_array());
    assert_eq!(meta.resources.len(), 2);
    assert_eq!(meta.resources[0].value["op"], "GET");
    assert_eq!(meta.resources[1].value["op"], "SET");
}

#[test]
fn a_missing_state_section_is_a_config_error() {
    let err = StatePackage::new(&Config::default())
        .err()
        .expect("missing section");
    assert!(err.to_string().contains("missing 'state' section"), "{err}");
}

#[test]
fn a_malformed_state_section_is_a_config_error() {
    let err = StatePackage::new(&config("[state]\ndatabase_uri = 5\n"))
        .err()
        .expect("bad section");
    assert!(err.to_string().contains("get state config error"), "{err}");
}

#[test]
fn an_invalid_database_uri_is_a_config_error() {
    let err = StatePackage::new(&config_with_uri("not a redis uri"))
        .err()
        .expect("bad uri");
    assert!(
        err.to_string().contains("create redis client error"),
        "{err}"
    );
}

#[test]
fn a_state_section_creates_the_client() {
    StatePackage::new(&config_with_uri("redis://127.0.0.1:6379")).expect("a valid uri is accepted");
}

// ---- a minimal RESP server ----

#[derive(Default)]
struct FakeRedis {
    values: HashMap<String, String>,
    /// When set, every GET answers this value instead of the stored one: a
    /// value the package cannot parse as JSON.
    raw_on_get: Option<String>,
    /// Answer PING with an error, as an unready server would.
    fail_ping: bool,
}

async fn start_fake_redis(store: Arc<Mutex<FakeRedis>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let store = store.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    buf.extend_from_slice(&chunk[..read]);
                    while let Some((args, used)) = parse_command(&buf) {
                        buf.drain(..used);
                        if socket.write_all(&respond(&args, &store)).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    format!("redis://{addr}")
}

fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    (from..buf.len().saturating_sub(1)).find(|&i| buf[i] == b'\r' && buf[i + 1] == b'\n')
}

/// Parse one RESP array-of-bulk-strings command, if the buffer holds a whole
/// one. Returns the arguments and how many bytes it took.
fn parse_command(buf: &[u8]) -> Option<(Vec<Vec<u8>>, usize)> {
    if buf.first()? != &b'*' {
        return None;
    }
    let line_end = find_crlf(buf, 0)?;
    let count: usize = std::str::from_utf8(&buf[1..line_end]).ok()?.parse().ok()?;
    let mut pos = line_end + 2;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        if buf.get(pos)? != &b'$' {
            return None;
        }
        let line_end = find_crlf(buf, pos)?;
        let len: usize = std::str::from_utf8(&buf[pos + 1..line_end])
            .ok()?
            .parse()
            .ok()?;
        let start = line_end + 2;
        let end = start + len;
        if buf.len() < end + 2 {
            return None;
        }
        args.push(buf[start..end].to_vec());
        pos = end + 2;
    }
    Some((args, pos))
}

fn bulk(value: &str) -> Vec<u8> {
    format!("${}\r\n{value}\r\n", value.len()).into_bytes()
}

fn respond(args: &[Vec<u8>], store: &Mutex<FakeRedis>) -> Vec<u8> {
    let command = args
        .first()
        .map(|arg| String::from_utf8_lossy(arg).to_ascii_uppercase())
        .unwrap_or_default();
    let mut store = store.lock();
    match command.as_str() {
        "PING" => {
            if store.fail_ping {
                b"-ERR server is not ready\r\n".to_vec()
            } else {
                b"+PONG\r\n".to_vec()
            }
        }
        "GET" => {
            let key = args
                .get(1)
                .map(|arg| String::from_utf8_lossy(arg).to_string())
                .unwrap_or_default();
            match store
                .raw_on_get
                .clone()
                .or_else(|| store.values.get(&key).cloned())
            {
                Some(value) => bulk(&value),
                None => b"$-1\r\n".to_vec(),
            }
        }
        "SET" => {
            if let (Some(key), Some(value)) = (args.get(1), args.get(2)) {
                store.values.insert(
                    String::from_utf8_lossy(key).to_string(),
                    String::from_utf8_lossy(value).to_string(),
                );
            }
            b"+OK\r\n".to_vec()
        }
        // CLIENT SETINFO and anything else the client may pipeline.
        _ => b"+OK\r\n".to_vec(),
    }
}

// ---- running one workflow act ----

/// Run `workflow` on an engine with the state package over `uri`; answer
/// whether it failed and the terminal event's vars (the act's outputs on
/// success, its `message` on failure).
async fn run(uri: &str, workflow: &Workflow) -> (bool, Vars) {
    let engine = Engine::builder()
        .set_config(&config_with_uri(uri))
        .add_package::<StatePackage>()
        .start()
        .await
        .unwrap();

    let ended = engine.signal::<Vec<(bool, Vars)>>(Vec::new());
    let error_ended = ended.clone();
    engine.channel().on_error(move |e| {
        let ended = error_ended.clone();
        let outputs = e.outputs.clone();
        async move {
            ended.update(|all| all.push((true, outputs.clone())));
            ended.close();
        }
    });
    let complete_ended = ended.clone();
    engine.channel().on_complete(move |e| {
        let ended = complete_ended.clone();
        let outputs = e.outputs.clone();
        async move {
            ended.update(|all| all.push((false, outputs.clone())));
            ended.close();
        }
    });

    let executor = engine.executor(&Principal::unrestricted());
    executor.model().deploy(workflow, None).await.unwrap();
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let mut all = ended.recv().await;
    all.pop().expect("the run reports one terminal event")
}

fn one_act(id: &str, params: serde_json::Value) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(
        Step::new()
            .with_id("s1")
            .with_uses("acts.app.state", params.into()),
    );
    workflow
}

fn state_store() -> Arc<Mutex<FakeRedis>> {
    Arc::new(Mutex::new(FakeRedis::default()))
}

// ---- the round trip ----

#[tokio::test(flavor = "multi_thread")]
async fn set_then_get_reads_the_value_back() {
    let uri = start_fake_redis(state_store()).await;
    let mut workflow = Workflow::new().with_id("state_roundtrip");
    workflow.steps.push(Step::new().with_id("set").with_uses(
        "acts.app.state",
        json!({ "op": "SET", "key": "greeting", "value": { "hello": "world" }, "params": { "key": "greeting", "value": { "hello": "world" } } }).into(),
    ));
    workflow.steps.push(Step::new().with_id("get").with_uses(
        "acts.app.state",
        json!({ "op": "GET", "key": "greeting", "params": { "key": "greeting" } }).into(),
    ));

    let (failed, outputs) = run(&uri, &workflow).await;
    assert!(!failed, "the round trip must complete: {outputs}");
    assert_eq!(
        outputs.get::<serde_json::Value>("greeting").unwrap(),
        json!({ "hello": "world" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_get_of_a_missing_key_fails() {
    let uri = start_fake_redis(state_store()).await;
    let (failed, outputs) = run(
        &uri,
        &one_act(
            "state_missing",
            json!({ "op": "GET", "key": "nope", "params": { "key": "nope" } }),
        ),
    )
    .await;
    assert!(failed, "a missing key cannot be read: {outputs}");
    assert!(
        outputs.to_string().contains("error happend to get value"),
        "{outputs}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unparseable_stored_value_fails_the_get() {
    let store = state_store();
    store.lock().raw_on_get = Some("not json".to_string());
    let uri = start_fake_redis(store).await;
    let (failed, outputs) = run(
        &uri,
        &one_act(
            "state_bad_json",
            json!({ "op": "GET", "key": "k", "params": { "key": "k" } }),
        ),
    )
    .await;
    assert!(failed, "an unparseable value cannot be read: {outputs}");
    assert!(
        outputs.to_string().contains("error happend to parse value"),
        "{outputs}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_get_requires_a_key() {
    let uri = start_fake_redis(state_store()).await;
    let (failed, outputs) = run(
        &uri,
        &one_act(
            "state_get_no_key",
            json!({ "op": "GET", "key": "k", "params": {} }),
        ),
    )
    .await;
    assert!(failed, "{outputs}");
    assert!(outputs.to_string().contains("missing 'key'"), "{outputs}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_set_requires_a_value() {
    let uri = start_fake_redis(state_store()).await;
    let (failed, outputs) = run(
        &uri,
        &one_act(
            "state_set_no_value",
            json!({ "op": "SET", "key": "k", "params": { "key": "k" } }),
        ),
    )
    .await;
    assert!(failed, "{outputs}");
    assert!(outputs.to_string().contains("missing 'value'"), "{outputs}");
}

#[tokio::test(flavor = "multi_thread")]
async fn params_missing_the_nested_section_are_refused() {
    let uri = start_fake_redis(state_store()).await;
    let (failed, outputs) = run(
        &uri,
        &one_act("state_bad_op", json!({ "op": "GET", "key": "k" })),
    )
    .await;
    assert!(failed, "{outputs}");
    assert!(
        outputs.to_string().contains("missing field `params`"),
        "{outputs}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_params_are_refused() {
    let uri = start_fake_redis(state_store()).await;
    let (failed, outputs) = run(
        &uri,
        &one_act(
            "state_bad_params",
            json!({ "op": "GET", "key": "k", "params": "not an object" }),
        ),
    )
    .await;
    assert!(failed, "{outputs}");
    assert!(
        outputs.to_string().contains("invalid ActPackage"),
        "{outputs}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_is_a_package_error() {
    // bind then drop: the address is refused from now on
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (failed, outputs) = run(
        &format!("redis://{addr}"),
        &one_act(
            "state_unreachable",
            json!({ "op": "GET", "key": "k", "params": { "key": "k" } }),
        ),
    )
    .await;
    assert!(failed, "{outputs}");
    assert!(
        outputs
            .to_string()
            .contains("error happend to connect redis"),
        "{outputs}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_ping_is_reported() {
    let store = state_store();
    store.lock().fail_ping = true;
    let uri = start_fake_redis(store).await;
    let (failed, outputs) = run(
        &uri,
        &one_act(
            "state_bad_ping",
            json!({ "op": "GET", "key": "k", "params": { "key": "k" } }),
        ),
    )
    .await;
    assert!(failed, "{outputs}");
    assert!(
        outputs.to_string().contains("error happend to ping redis"),
        "{outputs}"
    );
}
