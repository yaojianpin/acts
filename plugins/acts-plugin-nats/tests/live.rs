//! Live NATS tests for `acts-plugin-nats`.
//!
//! Requires a reachable NATS server: `ACTS_NATS_URL` (default
//! `nats://127.0.0.1:4222`). When no server responds the tests skip — keep
//! plain `cargo test` green without NATS. CI runs them against a NATS
//! service container.

use acts::{Config, Engine, Vars, Workflow};
use acts_acl::AclUsers;
use acts_plugin_nats::NatsPlugin;
use async_nats::Client;
use futures_util::StreamExt;
use serde_json::{Value as JsonValue, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

fn nats_url() -> String {
    std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string())
}

/// Connect and fail fast when no server is reachable (skip marker).
async fn connect_or_skip() -> Option<Client> {
    let url = nats_url();
    match tokio::time::timeout(Duration::from_secs(2), async_nats::connect(&url)).await {
        Ok(Ok(client)) => Some(client),
        _ => {
            eprintln!("skip: no NATS server reachable at {url}");
            None
        }
    }
}

/// Write a temporary acts config with the given `[nats]` section text.
///
/// The file name carries the process id and a counter rather than a timestamp:
/// the live tests run in parallel threads, and two of them landing on one
/// timestamp (the clock is coarser than a nanosecond on every platform) would
/// have one read the file the other is still writing.
fn temp_config(nats_section: &str) -> (PathBuf, Config) {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "acts_nats_test_{}_{}.toml",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(
        &path,
        format!("[log]\ndir = \"acts-nats-test-log\"\nlevel = \"INFO\"\n\n{nats_section}"),
    )
    .unwrap();
    let config = Config::create(&path).unwrap();
    (path, config)
}

/// The engine with access control on — the default — and the store-backed
/// registry installed: it is a separate crate, so without it a bare engine
/// refuses every `acl:*` action, `acl:login` and `set_user` included. The
/// anonymous caller behaves the same either way: catalogue-only.
async fn engine_with_nats(config: &Config) -> Engine {
    Engine::builder()
        .set_config(config)
        .with_user_acl()
        .add_plugin(&NatsPlugin::new())
        .start()
        .await
        .unwrap()
}

/// The same engine with access control explicitly off: the cases that use it
/// exercise the NATS action surface, so their caller must be unrestricted.
async fn engine_with_nats_unrestricted(config: &Config) -> Engine {
    Engine::builder()
        .set_config(config)
        .disable_acl()
        .add_plugin(&NatsPlugin::new())
        .start()
        .await
        .unwrap()
}

/// Publish one action and await its reply; retries while the plugin's
/// actions subscription is still starting up.
async fn request_action(client: &Client, subject: String, payload: JsonValue) -> JsonValue {
    let payload = serde_json::to_string(&payload).unwrap();
    for _ in 0..20 {
        match tokio::time::timeout(
            Duration::from_secs(2),
            client.request(subject.clone(), payload.clone().into()),
        )
        .await
        {
            Ok(Ok(msg)) => return serde_json::from_slice(&msg.payload).unwrap(),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    panic!("no reply from actions subject '{subject}'");
}

/// Each live test owns a distinct subject prefix: they share one broker and
/// run in parallel, so two engines subscribing to the same `<prefix>.cmd`
/// would answer each other's requests and fail on whichever engine answered.
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_actions_over_nats() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    // This case exercises the NATS action surface, so the caller must be
    // unrestricted (the default acl would answer it as anonymous, read-only).
    let (path, config) =
        temp_config("[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-snapshot-test\"\n");
    let engine = engine_with_nats_unrestricted(&config).await;

    // upsert
    let reply = request_action(
        &client,
        "acts-snapshot-test.cmd".to_string(),
        json!({
            "name": "snap:upsert",
            "seq": "req-1",
            "data": {
                "name": "profile",
                "scope": "u1",
                "rev": 5,
                "data": { "val": "x" }
            }
        }),
    )
    .await;
    assert_eq!(reply["data"], json!(true));
    assert_eq!(reply["ack"], "req-1");
    let entry = engine.snapshot().read("profile", "u1").unwrap();
    assert_eq!(entry.rev, 5);
    assert_eq!(entry.data.get::<String>("val").unwrap(), "x");

    // unknown action → err reply
    let reply = request_action(
        &client,
        "acts-snapshot-test.cmd".to_string(),
        json!({"name": "no:such", "seq": "x"}),
    )
    .await;
    assert!(
        reply["err"]
            .as_str()
            .unwrap()
            .contains("not found action 'no:such'")
    );
    assert_eq!(reply["data"], JsonValue::Null);

    // remove
    let reply = request_action(
        &client,
        "acts-snapshot-test.cmd".to_string(),
        json!({
            "name": "snap:remove",
            "seq": "req-2",
            "data": { "name": "profile", "scope": "u1" }
        }),
    )
    .await;
    assert_eq!(reply["data"], json!(true));
    assert!(engine.snapshot().read("profile", "u1").is_none());

    engine.close().await;
    std::fs::remove_file(&path).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_events_forwarded_to_nats() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let (path, config) = temp_config(
        "[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-events-test\"\n\n\
         [[nats.channels]]\n\
         id = \"t2\"\n\
         subject = \"acts-events-test.evt.t2\"\n\
         type = \"*\"\n\
         state = \"*\"\n\
         uses = \"*\"\n",
    );
    let engine = engine_with_nats(&config).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut sub = client.subscribe("acts-events-test.evt.t2").await.unwrap();
    let executor = engine.executor(&acts::Principal::unrestricted());
    let workflow = Workflow::new()
        .with_id("nats_event_demo")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses_code("acts.app.javascript", r#"return { output: "done" };"#)
        });
    executor.model().deploy(&workflow, None).await.unwrap();
    executor
        .proc()
        .start(&workflow.id, Vars::new().with("pid", "natev1"))
        .await
        .unwrap();

    // at least one engine message arrives on the channel subject
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timeout waiting for an engine event on nats")
        .expect("subscription ended");
    let envelope: JsonValue = serde_json::from_slice(&msg.payload).unwrap();
    // event envelope: seq carries the message id, data the full payload
    assert!(envelope["ack"].as_str().unwrap_or_default() != "");
    let data = envelope["data"]
        .as_object()
        .expect("event payload must be an object");
    assert!(data.contains_key("id"));
    assert_eq!(data["pid"], json!("natev1"));

    engine.close().await;
    std::fs::remove_file(&path).ok();
}

/// A non-object `data` must be rejected over the wire, never reinterpreted as
/// empty options: empty options on `msg:clear` clear every error delivery.
#[tokio::test(flavor = "multi_thread")]
async fn malformed_action_data_rejected() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let (path, config) =
        temp_config("[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-malformed-test\"\n");
    let engine = engine_with_nats_unrestricted(&config).await;

    for data in [json!([]), json!("msg:clear"), json!(7)] {
        let reply = request_action(
            &client,
            "acts-malformed-test.cmd".to_string(),
            json!({ "name": "msg:clear", "seq": "req-bad", "data": data }),
        )
        .await;
        assert_eq!(reply["ack"], "req-bad");
        assert_eq!(reply["data"], JsonValue::Null);
        assert!(
            reply["err"]
                .as_str()
                .unwrap_or_default()
                .contains("must be a JSON object"),
            "data={data} must be rejected: {reply}"
        );
    }

    // an object payload is still accepted
    let reply = request_action(
        &client,
        "acts-malformed-test.cmd".to_string(),
        json!({ "name": "msg:clear", "seq": "req-ok", "data": {} }),
    )
    .await;
    assert!(
        reply["err"].is_null(),
        "object data must be accepted: {reply}"
    );

    engine.close().await;
    std::fs::remove_file(&path).ok();
}

/// The action payload's `token` is the credential over NATS (the broker
/// authenticates a connection, not a request): a payload without one is
/// refused, and a user only gets the actions it was granted. Credentials live
/// in the store, not in the config file, so the token is what `acl:login`
/// answers over this same subject.
///
/// The test uses its own subject prefix: several live tests run in parallel
/// against one broker, and two engines subscribing to the same `<subject>.cmd`
/// would answer each other's requests.
#[tokio::test(flavor = "multi_thread")]
async fn acl_enforced_over_nats() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let (path, config) = temp_config(
        r#"
        [nats]
        url = "nats://127.0.0.1:4222"
        subject = "acts-acl-test"
        "#,
    );
    let engine = engine_with_nats(&config).await;
    engine
        .acl()
        .set_user(&acts::UserSpec {
            name: "operator".to_string(),
            add_passwords: vec!["op-pass".to_string()],
            allow: Some(vec!["msg:clear".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();

    // login over the wire: the reply's token is what the actions below carry
    let reply = request_action(
        &client,
        "acts-acl-test.cmd".to_string(),
        json!({
            "name": "acl:login",
            "seq": "req-login",
            "data": { "user": "operator", "password": "op-pass" }
        }),
    )
    .await;
    let token = reply["data"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("acl:login must answer a token: {reply}"))
        .to_string();

    let reply = request_action(
        &client,
        "acts-acl-test.cmd".to_string(),
        json!({ "name": "msg:clear", "seq": "req-anon", "data": {} }),
    )
    .await;
    assert!(
        reply["err"]
            .as_str()
            .unwrap_or_default()
            .contains("unauthenticated"),
        "a tokenless action must be refused: {reply}"
    );

    let reply = request_action(
        &client,
        "acts-acl-test.cmd".to_string(),
        json!({ "name": "model:rm", "seq": "req-denied", "data": { "id": "x" }, "token": token }),
    )
    .await;
    assert!(
        reply["err"]
            .as_str()
            .unwrap_or_default()
            .contains("permission denied"),
        "an action outside the user's grants must be denied: {reply}"
    );

    let reply = request_action(
        &client,
        "acts-acl-test.cmd".to_string(),
        json!({ "name": "msg:clear", "seq": "req-ok", "data": {}, "token": token }),
    )
    .await;
    assert!(
        reply["err"].is_null(),
        "an allowed action must run: {reply}"
    );

    engine.close().await;
    std::fs::remove_file(&path).ok();
}

/// A busy actions subject must not turn into unbounded work: past
/// `max_in_flight` an action is refused to its caller instead of being started,
/// and every request is still answered — the caller never hangs, and never has
/// to guess whether its action ran.
///
/// The burst shares one reply inbox so the replies can be counted. With
/// `max_in_flight = 1` one admitted action (a store read) takes far longer than
/// the loop needs to pull the next message, so almost every message of the
/// burst is refused while the first one still runs.
#[tokio::test(flavor = "multi_thread")]
async fn actions_beyond_the_in_flight_bound_are_refused() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let subject = "acts-inflight-test.cmd".to_string();
    let (path, config) = temp_config(
        "[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-inflight-test\"\nmax_in_flight = 1\n",
    );
    let engine = engine_with_nats_unrestricted(&config).await;

    // wait for the actions subscription to be live: its reply must be a run,
    // not a refusal
    let reply = request_action(
        &client,
        subject.clone(),
        json!({"name": "model:ls", "seq": "req-warmup"}),
    )
    .await;
    assert!(
        reply["err"].is_null(),
        "the subscription must be live: {reply}"
    );

    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await.unwrap();
    let burst = 256;
    for i in 0..burst {
        let payload = serde_json::to_vec(&json!({
            "name": "model:ls",
            "seq": format!("req-{i}"),
        }))
        .unwrap();
        client
            .publish_with_reply(subject.clone(), inbox.clone(), payload.into())
            .await
            .unwrap();
    }
    client.flush().await.unwrap();

    // every request of the burst answers: the refused ones with `err`, the
    // admitted ones with their action's result
    let mut answered: HashMap<String, JsonValue> = HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while answered.len() < burst && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), replies.next()).await {
            Ok(Some(msg)) => {
                let reply: JsonValue = serde_json::from_slice(&msg.payload).unwrap();
                let seq = reply["ack"].as_str().unwrap().to_string();
                answered.insert(seq, reply);
            }
            _ => break,
        }
    }
    assert_eq!(
        answered.len(),
        burst,
        "every request must be answered, refused ones included"
    );

    let refused = answered
        .values()
        .filter(|reply| {
            reply["err"]
                .as_str()
                .unwrap_or_default()
                .contains("too many actions in flight")
        })
        .count();
    assert!(
        refused > 0,
        "a burst past max_in_flight must be refused instead of queued: {refused} of {burst} refused"
    );
    assert!(
        refused < burst,
        "an action under the bound must still run: {refused} of {burst} refused"
    );

    engine.close().await;
    std::fs::remove_file(&path).ok();
}
