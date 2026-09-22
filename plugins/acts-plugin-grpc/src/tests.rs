use crate::{GrpcConfig, GrpcPlugin, GrpcServer};
use acts::query::Query as StoreQuery;
use acts::{ChannelOptions, Engine, Vars, Workflow};
use acts_proto::{
    Message, MessageOptions, acts_service_client::ActsServiceClient,
    acts_service_server::ActsService,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_stream::StreamExt as _;

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
    assert_eq!(
        config.queue_size(),
        crate::DEFAULT_QUEUE_SIZE,
        "an unconfigured subscription queue keeps the default capacity"
    );
}

/// The configured queue capacity is what the subscription is built with, and
/// `0` is clamped to a queue of one: a zero-capacity queue would open a stream
/// that can never receive a message.
#[test]
fn test_grpc_config_queue_size_is_honored_and_clamped() {
    let config: GrpcConfig = serde_json::from_value(json!({"queue_size": 8})).unwrap();
    assert_eq!(config.queue_size(), 8);

    let config: GrpcConfig = serde_json::from_value(json!({"queue_size": 0})).unwrap();
    assert_eq!(config.queue_size(), 1);
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

/// A transport engine with access control explicitly off: these cases exercise
/// the gRPC surface, so their caller must be unrestricted. The ACL cases below
/// build their own policies.
async fn engine_with_grpc(port: u16) -> Engine {
    engine_with_grpc_queue(port, None).await
}

/// The same engine with `[grpc].queue_size` pinned — the slow-subscriber case
/// needs a queue one run can fill.
async fn engine_with_grpc_queue(port: u16, queue_size: Option<usize>) -> Engine {
    let mut table = format!("[acl]\nenabled = false\n[grpc]\nport = {port}\n");
    if let Some(queue_size) = queue_size {
        table.push_str(&format!("queue_size = {queue_size}\n"));
    }
    let table: toml::Table = toml::from_str(&table).unwrap();
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

/// The wire path end to end: a generated client, the plugin's service, and the
/// engine behind it — including the graceful shutdown that must release the
/// port the plugin bound.
#[tokio::test(flavor = "multi_thread")]
async fn test_snapshot_upsert_remove_over_grpc() {
    let port = free_port();
    let engine = engine_with_grpc(port).await;

    // connect the generated client and wait until the server accepts
    let url = format!("http://127.0.0.1:{port}");
    let mut client = loop {
        match ActsServiceClient::connect(url.clone()).await {
            Ok(client) => break client,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };

    let answer = client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 5, "data": {"val": "x"}}),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(payload(&answer), json!(true));
    let entry = engine.snapshot().read("profile", "u1").unwrap();
    assert_eq!(entry.rev, 5);
    assert_eq!(entry.data.get::<String>("val").unwrap(), "x");

    let answer = client
        .send(action(
            "snap:remove",
            json!({"name": "profile", "scope": "u1"}),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(payload(&answer), json!(true));
    assert!(engine.snapshot().read("profile", "u1").is_none());

    // graceful shutdown stops the transport: the port must be released
    engine.close().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "engine.close() must stop the gRPC server and release its port"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_grpc_server_new() {
    let engine = Engine::builder().start().await.unwrap();
    let server = GrpcServer::new(&engine);
    let _ = server;
    engine.close().await;
}

/// `data` is the action payload and must be a JSON object. A malformed
/// payload is rejected as INVALID_ARGUMENT instead of silently becoming empty
/// options — empty options on `msg:clear` mean "clear every error delivery".
#[tokio::test(flavor = "multi_thread")]
async fn test_do_action_rejects_malformed_data() {
    let engine = engine_with_grpc(free_port()).await;
    let server = GrpcServer::new(&engine);

    let malformed: [&[u8]; 4] = [b"[]", b"{", b"null", b"\"msg:clear\""];
    for data in malformed {
        let message = Message {
            name: "msg:clear".to_string(),
            seq: "seq-1".to_string(),
            ack: None,
            data: Some(data.to_vec()),
        };
        let status = server
            .do_action(message, None)
            .await
            .expect_err("malformed data must be rejected");
        assert_eq!(
            status.code(),
            tonic::Code::InvalidArgument,
            "data={data:?} must be INVALID_ARGUMENT"
        );
    }

    // nothing was applied: the rejected `msg:clear` must not have cleared
    let stored = engine
        .executor(&acts::Principal::unrestricted())
        .msg()
        .list(&StoreQuery::new().offset(0).limit(10))
        .await
        .unwrap()
        .count;
    assert_eq!(stored, 0);

    // absent or object-valued data stays valid
    for data in [None, Some(b"{}".to_vec())] {
        let message = Message {
            name: "msg:clear".to_string(),
            seq: "seq-1".to_string(),
            ack: None,
            data,
        };
        let response = server
            .do_action(message, None)
            .await
            .expect("absent or object data must be accepted")
            .into_inner();
        assert_eq!(response.ack.as_deref(), Some("seq-1"));
    }

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
        .executor(&acts::Principal::unrestricted())
        .model()
        .deploy(&model, None)
        .await
        .unwrap();
    engine
        .executor(&acts::Principal::unrestricted())
        .proc()
        .start(&model.id, Vars::new())
        .await
        .unwrap();
}

async fn stored_message_count(engine: &Engine) -> usize {
    engine
        .executor(&acts::Principal::unrestricted())
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
    let engine = engine_with_grpc(free_port()).await;
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

/// A subscriber that stops reading must be disconnected, not served forever:
/// `MessageClient::send` writes into a bounded queue and never waits for room,
/// so the queue is the whole backlog of an RPC — once it is full the stream
/// ends instead of growing a waiting task per message. Nothing is lost: the
/// delivery stays unacked, so the engine's retry timer re-sends it when the
/// client subscribes again.
#[tokio::test(flavor = "multi_thread")]
async fn test_on_message_slow_subscriber_is_disconnected() {
    // a two-message queue: one run already emits past the bound
    let engine = engine_with_grpc_queue(free_port(), Some(2)).await;
    let server = GrpcServer::new(&engine);

    // spy channel: counts every dispatched workflow message without storing
    // anything (ack = false), so the emissions are observable while the
    // subscription under test is left unread
    let received = Arc::new(AtomicUsize::new(0));
    let spy = engine.channel_with_options(&ChannelOptions {
        ack: false,
        ..Default::default()
    });
    let spy_received = received.clone();
    spy.on_message(move |_e| {
        let received = spy_received.clone();
        async move {
            received.fetch_add(1, Ordering::Relaxed);
        }
    });

    // subscribe like a gRPC client would, and never read the stream
    let options = MessageOptions {
        client_id: "slow-reader".to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    };
    let response = server
        .on_message(tonic::Request::new(options))
        .await
        .expect("the subscription opens");

    // two runs emit at least 6 messages, so the queue (2) is exceeded even
    // while the count lags the handler under test by the message in flight
    run_irq_workflow(&engine, "slow1").await;
    run_irq_workflow(&engine, "slow2").await;
    let seen = || received.load(Ordering::Relaxed);
    wait_until(|| seen() >= 6, "workflow messages to be dispatched").await;

    // the overflow ended the RPC: the stream flushes what was queued and is
    // done, instead of waiting for a reader that never came
    let mut stream = response.into_inner();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(status))) => panic!("subscription failed: {status}"),
            Ok(None) => break,
            Err(_) => {
                panic!("a subscriber that stopped reading must be disconnected, not left queued")
            }
        }
    }

    // and the channel is deregistered: later messages are not delivered to
    // (and not stored for) the closed subscription
    let rows = stored_message_count(&engine).await;
    run_irq_workflow(&engine, "after").await;
    wait_until(
        || seen() >= 9,
        "messages of the last workflow to be dispatched",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        stored_message_count(&engine).await,
        rows,
        "a disconnected gRPC channel must not store further deliveries"
    );

    engine.close().await;
}

/// An engine whose `[acl]` section is `acl_text`.
async fn engine_with_acl(acl_text: &str) -> Engine {
    let config: toml::Table = toml::from_str(acl_text).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table: config,
    };
    Engine::builder().set_config(&cfg).start().await.unwrap()
}

const GRPC_ACL: &str = r#"
[acl]

[[acl.role]]
name = "operator"
tokens = ["op-token"]
allow = ["msg:clear", "msg:sub"]
"#;

fn send_request(name: &str, token: Option<&str>) -> tonic::Request<Message> {
    let mut request = tonic::Request::new(Message {
        name: name.to_string(),
        seq: "seq-1".to_string(),
        ack: None,
        data: Some(b"{}".to_vec()),
    });
    if let Some(token) = token {
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    request
}

/// One `Send` request carrying `options` as the action payload.
fn action(name: &str, options: serde_json::Value) -> tonic::Request<Message> {
    tonic::Request::new(Message {
        name: name.to_string(),
        seq: "seq-1".to_string(),
        ack: None,
        data: Some(serde_json::to_vec(&options).unwrap()),
    })
}

/// The action's own value, decoded from an answer message.
fn payload(answer: &Message) -> serde_json::Value {
    let data = answer
        .data
        .as_deref()
        .expect("the answer carries a payload");
    serde_json::from_slice(data).expect("the answer payload is json")
}

/// The `authorization: Bearer <token>` metadata is the credential: a request
/// without one is UNAUTHENTICATED, and a role only gets the actions it was
/// granted.
#[tokio::test(flavor = "multi_thread")]
async fn test_send_enforces_acl() {
    let engine = engine_with_acl(GRPC_ACL).await;
    let server = GrpcServer::new(&engine);

    let status = server
        .send(send_request("msg:clear", None))
        .await
        .expect_err("no token must be refused");
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status}");

    let status = server
        .send(send_request("msg:clear", Some("bogus")))
        .await
        .expect_err("an unknown token must be refused");
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status}");

    let status = server
        .send(send_request("model:rm", Some("op-token")))
        .await
        .expect_err("an action outside the role must be refused");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status}");

    server
        .send(send_request("msg:clear", Some("op-token")))
        .await
        .expect("an allowed action must run");

    // the subscribe stream is closed to an anonymous caller too
    let anonymous = server
        .on_message(tonic::Request::new(MessageOptions::default()))
        .await
        .err()
        .expect("an anonymous subscription must be refused");
    assert_eq!(anonymous.code(), tonic::Code::Unauthenticated);

    let mut request = tonic::Request::new(MessageOptions::default());
    request
        .metadata_mut()
        .insert("authorization", "Bearer op-token".parse().unwrap());
    server
        .on_message(request)
        .await
        .expect("an authenticated subscription must open");
}

/// Two tenants, both allowed to start runs, subscribe and ack.
const SUB_ACL: &str = r#"
[acl]

[[acl.role]]
name = "u1"
tokens = ["token-u1"]
allow = ["model:deploy", "proc:start", "msg:ack", "msg:sub"]

[[acl.role]]
name = "u2"
tokens = ["token-u2"]
allow = ["model:deploy", "proc:start", "msg:ack", "msg:sub"]
"#;

fn subscribe(client_id: &str, token: &str) -> tonic::Request<MessageOptions> {
    let mut request = tonic::Request::new(MessageOptions {
        client_id: client_id.to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    });
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

/// Start the deployed `mid` as the principal's own run and answer its pid.
async fn start_owned(engine: &Engine, principal: &acts::Principal, mid: &str) -> String {
    acts::actions::apply_as(engine, principal, "proc:start", Vars::new().with("id", mid))
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

/// Collect what a subscription stream carries until it goes quiet for
/// `idle_ms`, decoding each wire payload back into an `acts::Message`.
async fn drain<T>(stream: &mut T, idle_ms: u64) -> Vec<acts::Message>
where
    T: tokio_stream::Stream<Item = Result<Message, tonic::Status>> + Unpin,
{
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(idle_ms), stream.next()).await {
            Ok(Some(Ok(message))) => {
                let data = message.data.expect("a delivered message carries data");
                out.push(serde_json::from_slice::<acts::Message>(&data).unwrap());
            }
            Ok(Some(Err(status))) => panic!("subscription failed: {status}"),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    out
}

/// The `client_id` is not the channel key on its own: the subject namespaces
/// it, so two subscribers naming the same id coexist instead of one replacing
/// the other's handler. Both see every process — delivery follows the grants
/// and the channel filters, not the run's starter.
#[tokio::test(flavor = "multi_thread")]
async fn test_subscriptions_are_namespaced_by_subject() {
    let engine = engine_with_acl(SUB_ACL).await;
    let server = GrpcServer::new(&engine);
    let u1 = engine.acl().authenticate(Some("token-u1")).unwrap();
    let u2 = engine.acl().authenticate(Some("token-u2")).unwrap();

    let model = Workflow::new().with_id("grpc-scope").with_step(|step| {
        step.with_id("step1")
            .with_uses("acts.core.irq", Vars::new().with("key", "scope-test"))
    });
    for principal in [&u1, &u2] {
        acts::actions::apply_as(
            &engine,
            principal,
            "model:deploy",
            Vars::new().with("model", model.to_yml().unwrap()),
        )
        .await
        .unwrap();
    }

    // the same client id on both sides, a different token each
    let mut u1_stream = server
        .on_message(subscribe("shared-client", "token-u1"))
        .await
        .unwrap()
        .into_inner();
    let mut u2_stream = server
        .on_message(subscribe("shared-client", "token-u2"))
        .await
        .unwrap()
        .into_inner();

    let u1_pid = start_owned(&engine, &u1, "grpc-scope").await;
    let u2_pid = start_owned(&engine, &u2, "grpc-scope").await;

    let seen_u1 = drain(&mut u1_stream, 1_000).await;
    let seen_u2 = drain(&mut u2_stream, 1_000).await;

    // Both registrations are live — neither handler replaced the other — and
    // each stream carried both runs.
    assert!(!seen_u1.is_empty(), "u1's subscription received nothing");
    assert!(!seen_u2.is_empty(), "u2's subscription received nothing");
    for (subject, seen) in [("u1", &seen_u1), ("u2", &seen_u2)] {
        assert!(
            seen.iter().any(|m| m.pid == u1_pid),
            "{subject}'s stream never saw u1's run"
        );
        assert!(
            seen.iter().any(|m| m.pid == u2_pid),
            "{subject}'s stream never saw u2's run"
        );
    }

    engine.close().await;
}

/// A subscription is an action: a role without `msg:sub` is refused the
/// stream with PERMISSION_DENIED, and one with it is granted a channel.
#[tokio::test(flavor = "multi_thread")]
async fn test_subscription_needs_the_grant() {
    let engine = engine_with_acl(SUB_ACL).await;
    let server = GrpcServer::new(&engine);

    // `msg:ls` is a read of stored messages; it is not a subscription grant.
    let engine2 = engine_with_acl(
        r#"
        [acl]
        [[acl.role]]
        name = "reader"
        tokens = ["reader-token"]
        allow = ["msg:ls"]
        "#,
    )
    .await;
    let reader = GrpcServer::new(&engine2);
    let status = reader
        .on_message(subscribe("reader-client", "reader-token"))
        .await
        .err()
        .expect("a role without msg:sub must not open a stream");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status}");
    engine2.close().await;

    let stream = server
        .on_message(subscribe("granted-client", "token-u1"))
        .await
        .expect("msg:sub opens the stream");
    drop(stream);
    engine.close().await;
}
