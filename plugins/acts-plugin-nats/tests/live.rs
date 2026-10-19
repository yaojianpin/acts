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

/// One raw (non-JSON) publish to the actions subject with a reply inbox: the
/// plugin must answer it with an error envelope instead of dropping the
/// request — and must not treat the unparsable message as a run.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_payload_is_answered() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let subject = "acts-invalid-test.cmd".to_string();
    let (path, config) =
        temp_config("[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-invalid-test\"\n");
    let engine = engine_with_nats_unrestricted(&config).await;

    // warm up: the subscription is live once a well-formed request answers
    let reply = request_action(
        &client,
        subject.clone(),
        json!({"name": "model:ls", "seq": "warmup"}),
    )
    .await;
    assert!(reply["err"].is_null(), "{reply}");

    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await.unwrap();
    client
        .publish_with_reply(subject, inbox, "not json".as_bytes().to_vec().into())
        .await
        .unwrap();
    client.flush().await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), replies.next())
        .await
        .expect("a malformed payload must be answered")
        .expect("the reply subscription is live");
    let reply: JsonValue = serde_json::from_slice(&msg.payload).unwrap();
    assert!(
        reply["err"]
            .as_str()
            .unwrap_or_default()
            .contains("invalid payload"),
        "a malformed payload must be refused: {reply}"
    );

    // and the same message without a reply inbox is simply dropped
    client
        .publish(
            "acts-invalid-test.cmd",
            "still not json".as_bytes().to_vec().into(),
        )
        .await
        .unwrap();
    client.flush().await.unwrap();
    // a following well-formed request is answered only after the malformed
    // one (no reply inbox) was consumed
    let reply = request_action(
        &client,
        "acts-invalid-test.cmd".to_string(),
        json!({"name": "model:ls", "seq": "after-malformed"}),
    )
    .await;
    assert!(reply["err"].is_null(), "{reply}");

    engine.close().await;
    std::fs::remove_file(&path).ok();
}

/// Each channel's subject and id are optional: the subject defaults to
/// `<config.subject>.evt.<id>`, and the id to the subject. Both defaulted
/// shapes register a live channel.
#[tokio::test(flavor = "multi_thread")]
async fn channel_subject_and_id_defaults() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let (path, config) = temp_config(
        "[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-defaults-test\"\n\n\
         [[nats.channels]]\n\
         id = \"by-id\"\n\
         type = \"*\"\n\
         state = \"*\"\n\
         uses = \"*\"\n\
         [nats.channels.options]\n\
         pid = \"*\"\n\n\
         [[nats.channels]]\n\
         subject = \"acts-defaults-test.evt.explicit\"\n",
    );
    let engine = engine_with_nats_unrestricted(&config).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut by_id = client
        .subscribe("acts-defaults-test.evt.by-id")
        .await
        .unwrap();
    let mut explicit = client
        .subscribe("acts-defaults-test.evt.explicit")
        .await
        .unwrap();

    let executor = engine.executor(&acts::Principal::unrestricted());
    let workflow = Workflow::new()
        .with_id("nats_defaults_demo")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses_code("acts.app.javascript", r#"return { output: "done" };"#)
        });
    executor.model().deploy(&workflow, None).await.unwrap();
    executor
        .proc()
        .start(&workflow.id, Vars::new().with("pid", "natdefaults"))
        .await
        .unwrap();

    for (label, sub) in [
        ("defaulted subject", &mut by_id),
        ("defaulted id", &mut explicit),
    ] {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("no event on the {label} channel"))
            .expect("the subscription ended");
        let envelope: JsonValue = serde_json::from_slice(&msg.payload).unwrap();
        assert!(
            envelope["ack"].as_str().unwrap_or_default() != "",
            "the {label} channel carries the message id: {envelope}"
        );
    }

    engine.close().await;
    std::fs::remove_file(&path).ok();
}

/// A broker that cannot be reached disables the plugin instead of failing the
/// engine: the transport is optional, and its connect errors are logged. Both
/// credential shapes (token, user/password) take the same path.
#[tokio::test(flavor = "multi_thread")]
async fn unreachable_broker_leaves_the_engine_running() {
    for credentials in [
        "token = \"secret\"\n",
        "username = \"u\"\npassword = \"p\"\n",
    ] {
        let (path, config) = temp_config(&format!(
            "[nats]\nurl = \"nats://127.0.0.1:1\"\n{credentials}"
        ));
        let engine = engine_with_nats_unrestricted(&config).await;
        // the connect attempt fails in the plugin's background task
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        let models = engine
            .executor(&acts::Principal::unrestricted())
            .model()
            .list(&acts::query::Query::new().limit(1))
            .await
            .expect("the engine serves local calls while NATS is unreachable");
        assert_eq!(models.count, 0);
        engine.close().await;
        std::fs::remove_file(&path).ok();
    }
}

/// An access control whose `authenticate` always refuses: the shipped
/// registries answer a tokenless request with the anonymous principal, so a
/// caller the broker cannot authenticate has no other way to be refused.
#[derive(Debug)]
struct RefusingAcl;

impl RefusingAcl {
    fn failure() -> acts::AclError {
        acts::AclError::Unauthenticated("no such session".to_string())
    }
}

#[async_trait::async_trait]
impl acts::AccessControl for RefusingAcl {
    async fn load(&self, _store: std::sync::Arc<acts::Store>) -> acts::Result<()> {
        Ok(())
    }

    fn enabled(&self) -> bool {
        true
    }

    fn authenticate(
        &self,
        _token: Option<&str>,
    ) -> std::result::Result<acts::Principal, acts::AclError> {
        Err(Self::failure())
    }

    async fn login(
        &self,
        _user: &str,
        _password: &str,
    ) -> std::result::Result<acts::LoginTokens, acts::AclError> {
        Err(Self::failure())
    }

    async fn refresh(
        &self,
        _refresh_token: &str,
    ) -> std::result::Result<acts::LoginTokens, acts::AclError> {
        Err(Self::failure())
    }

    async fn logout(&self, _token: &str) -> acts::Result<bool> {
        Err(acts::ActError::Unauthenticated("no session".to_string()))
    }

    async fn set_user(&self, _spec: &acts::UserSpec) -> acts::Result<()> {
        Err(acts::ActError::Denied("not allowed".to_string()))
    }

    async fn del_user(&self, _name: &str) -> acts::Result<()> {
        Err(acts::ActError::Denied("not allowed".to_string()))
    }

    async fn user_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn get_user(&self, _name: &str) -> Option<JsonValue> {
        None
    }
}

/// A credential the ACL cannot resolve is refused to its caller as an error
/// envelope, whether or not the request named a reply inbox — and a message
/// without a reply is dropped after the refusal instead of wedging the loop.
#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_action_is_refused() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let subject = "acts-refuse-test.cmd".to_string();
    let (path, config) =
        temp_config("[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts-refuse-test\"\n");
    let engine = Engine::builder()
        .set_config(&config)
        .set_acl(std::sync::Arc::new(RefusingAcl))
        .add_plugin(&NatsPlugin::new())
        .start()
        .await
        .unwrap();

    // with a reply inbox: the refusal is answered
    let reply = request_action(
        &client,
        subject.clone(),
        json!({"name": "model:ls", "seq": "req-anon"}),
    )
    .await;
    assert!(
        reply["err"]
            .as_str()
            .unwrap_or_default()
            .contains("unauthenticated"),
        "an unauthenticated action must be refused: {reply}"
    );
    assert_eq!(reply["ack"], "req-anon");

    // without one: the message is still consumed (a following request gets
    // its own answer), so it cannot wedge the actions subscription
    client
        .publish(
            subject.clone(),
            serde_json::to_vec(&json!({"name": "model:ls"}))
                .unwrap()
                .into(),
        )
        .await
        .unwrap();
    client.flush().await.unwrap();
    let reply = request_action(
        &client,
        subject,
        json!({"name": "model:ls", "seq": "req-after"}),
    )
    .await;
    assert_eq!(reply["ack"], "req-after");

    engine.close().await;
    std::fs::remove_file(&path).ok();
}
