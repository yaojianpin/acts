use crate::{DEFAULT_HOST, GrpcConfig, GrpcPlugin, GrpcServer};

/// Install a sink subscriber once per test binary: tracing's field
/// expressions are evaluated only when a subscriber is interested, so a
/// logged value (e.g. the `authenticated` flag) would not run otherwise.
fn init_tracing() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(std::io::sink)
            .try_init();
    });
}
use acts::query::Query as StoreQuery;
use acts::{ChannelOptions, Engine, Vars, Workflow};
use acts_acl::AclUsers;
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
    let plugin = GrpcPlugin::default();
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
/// declare their own users.
async fn engine_with_grpc(port: u16) -> Engine {
    engine_with_grpc_queue(port, None).await
}

/// The same engine with `[grpc].queue_size` pinned — the slow-subscriber case
/// needs a queue one run can fill.
async fn engine_with_grpc_queue(port: u16, queue_size: Option<usize>) -> Engine {
    let mut table = format!("[grpc]\nport = {port}\n");
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
        .disable_acl()
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

/// The gRPC action path carries the assignment actions to the same shared
/// table: transfer, delegate and undelegate run against the act, and the
/// act's state is untouched (the decisions still complete it).
#[tokio::test(flavor = "multi_thread")]
async fn test_assignment_actions_over_grpc() {
    let port = free_port();
    let engine = engine_with_grpc(port).await;
    let (pid, tid) = start_assigned(&engine, "grpc-act-actions").await;

    let url = format!("http://127.0.0.1:{port}");
    let mut client = loop {
        match ActsServiceClient::connect(url.clone()).await {
            Ok(client) => break client,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };

    let answer = client
        .send(action(
            "act:transfer",
            json!({"pid": &pid, "tid": &tid, "to": "bob", "expect_rev": 0}),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(payload(&answer), json!(null));

    let answer = client
        .send(action(
            "act:delegate",
            json!({
                "pid": &pid,
                "tid": &tid,
                "delegatee": "carol",
                "expires_at": 1_761_000_000_000i64,
                "reason": "holiday"
            }),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(payload(&answer), json!(null));

    let answer = client
        .send(action("act:undelegate", json!({"pid": &pid, "tid": &tid})))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(payload(&answer), json!(null));

    // The act itself was never decided by the assignment actions.
    let answer = client
        .send(action(
            "act:complete",
            json!({"pid": &pid, "tid": &tid, "rev": 3}),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(payload(&answer), json!(null));
    engine.close().await;
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
            .do_action(message, None, None)
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
            .do_action(message, None, None)
            .await
            .expect("absent or object data must be accepted")
            .into_inner();
        assert_eq!(response.ack.as_deref(), Some("seq-1"));
    }

    engine.close().await;
}

/// Deploy and start one assigned irq act, answering `(pid, tid)` from its
/// Created message. The channel is registered before the start, so the message
/// cannot be missed.
async fn start_assigned(engine: &Engine, id: &str) -> (String, String) {
    let workflow = Workflow::new().with_id(id).with_step(|step| {
        step.with_id("step1")
            .with_uses("acts.core.irq", Vars::new().with("key", "grpc-act-test"))
            .with_owner("alice")
    });
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
        .executor(&acts::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();
    let pid = engine
        .executor(&acts::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap()
        .pid()
        .to_string();
    let created = rx.recv().await.expect("the act's Created message");
    (pid, created.tid)
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
        || *received.lock().unwrap() >= 2,
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
        || *received.lock().unwrap() >= 4,
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

    // two runs emit four messages, so the queue (2) is exceeded even while
    // the count lags the handler under test by the message in flight
    run_irq_workflow(&engine, "slow1").await;
    run_irq_workflow(&engine, "slow2").await;
    let seen = || received.load(Ordering::Relaxed);
    wait_until(|| seen() >= 4, "workflow messages to be dispatched").await;

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
        || seen() >= 6,
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

/// An engine with access control on — the default — and one user per
/// `(name, password, allow)` entry. Credentials live in the store, not in the
/// config file, so a request carries the session token `login` answers. The
/// store-backed registry is a separate crate: without it a bare engine refuses
/// every `acl:*` action, `acl:login` and `set_user` included.
async fn engine_with_users(users: &[(&str, &str, &[&str])]) -> Engine {
    let engine = Engine::builder().with_user_acl().start().await.unwrap();
    for (name, password, allow) in users {
        engine
            .acl()
            .set_user(&acts::UserSpec {
                name: (*name).to_string(),
                add_passwords: vec![(*password).to_string()],
                allow: Some(allow.iter().map(|a| (*a).to_string()).collect()),
                // every case here deploys or starts a model, so the users own
                // the `*` resource — the model's own `rn` must still be set
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
/// without one (or with an unknown one) is UNAUTHENTICATED, and a user only
/// gets the actions it was granted.
#[tokio::test(flavor = "multi_thread")]
async fn test_send_enforces_acl() {
    let engine = engine_with_users(&[("operator", "op-pass", &["msg:clear", "msg:sub"])]).await;
    let server = GrpcServer::new(&engine);

    // `acl:login` needs no grant — the credentials are its payload — and the
    // token it answers is what every request below presents
    let answer = server
        .send(action(
            "acl:login",
            json!({"user": "operator", "password": "op-pass"}),
        ))
        .await
        .expect("acl:login must run without a credential")
        .into_inner();
    let token = payload(&answer)["token"]
        .as_str()
        .expect("acl:login must answer a session token")
        .to_string();

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
        .send(send_request("model:rm", Some(&token)))
        .await
        .expect_err("an action outside the user's grants must be refused");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status}");

    server
        .send(send_request("msg:clear", Some(&token)))
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
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    server
        .on_message(request)
        .await
        .expect("an authenticated subscription must open");
}

/// A bearer that no longer resolves still reaches the actions whose payload is
/// the credential: refusing them would make the very request that repairs a
/// dead session impossible. Every other action stays UNAUTHENTICATED.
#[tokio::test(flavor = "multi_thread")]
async fn test_stale_bearer_reaches_the_credential_actions() {
    let engine = engine_with_users(&[("operator", "op-pass", &[])]).await;
    let server = GrpcServer::new(&engine);

    // the login is answered even though the bearer in hand is invalid
    let mut login_request = action(
        "acl:login",
        json!({"user": "operator", "password": "op-pass"}),
    );
    login_request
        .metadata_mut()
        .insert("authorization", "Bearer bogus".parse().unwrap());
    let answer = server
        .send(login_request)
        .await
        .expect("acl:login carries its own credential")
        .into_inner();
    let refresh_token = payload(&answer)["refresh_token"]
        .as_str()
        .expect("acl:login must answer a refresh token")
        .to_string();

    // and so is the rotation that replaces the dead session
    let mut refresh_request = action("acl:refresh", json!({"refresh_token": refresh_token}));
    refresh_request
        .metadata_mut()
        .insert("authorization", "Bearer bogus".parse().unwrap());
    server
        .send(refresh_request)
        .await
        .expect("acl:refresh carries its own credential");

    // the identity answer is about the bearer, so an invalid one is refused
    let status = server
        .send(send_request("acl:whoami", Some("bogus")))
        .await
        .expect_err("whoami needs a credential that resolves");
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status}");
}

/// Two tenants, both allowed to start runs, subscribe and ack.
const SUB_ALLOW: &[&str] = &["model:deploy", "proc:start", "msg:ack", "msg:sub"];

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
    let engine =
        engine_with_users(&[("u1", "u1-pass", SUB_ALLOW), ("u2", "u2-pass", SUB_ALLOW)]).await;
    let server = GrpcServer::new(&engine);
    let u1_token = login(&engine, "u1", "u1-pass").await;
    let u2_token = login(&engine, "u2", "u2-pass").await;
    let u1 = engine.acl().authenticate(Some(&u1_token)).unwrap();
    let u2 = engine.acl().authenticate(Some(&u2_token)).unwrap();

    // the model names a resource both users own (`patterns = ["*"]`): a model
    // without an `rn` may only be deployed by an unrestricted user
    let model = Workflow::new()
        .with_id("grpc-scope")
        .with_rn("grpc:scope")
        .with_step(|step| {
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

    // the same client id on both sides, a different session each
    let mut u1_stream = server
        .on_message(subscribe("shared-client", &u1_token))
        .await
        .unwrap()
        .into_inner();
    let mut u2_stream = server
        .on_message(subscribe("shared-client", &u2_token))
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

/// A subscription is an action: a user without `msg:sub` is refused the
/// stream with PERMISSION_DENIED, and one with it is granted a channel.
#[tokio::test(flavor = "multi_thread")]
async fn test_subscription_needs_the_grant() {
    let engine = engine_with_users(&[("u1", "u1-pass", SUB_ALLOW)]).await;
    let server = GrpcServer::new(&engine);

    // `msg:ls` is a read of stored messages; it is not a subscription grant.
    let engine2 = engine_with_users(&[("reader", "reader-pass", &["msg:ls"])]).await;
    let reader = GrpcServer::new(&engine2);
    let reader_token = login(&engine2, "reader", "reader-pass").await;
    let status = reader
        .on_message(subscribe("reader-client", &reader_token))
        .await
        .err()
        .expect("a user without msg:sub must not open a stream");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status}");
    engine2.close().await;

    let u1_token = login(&engine, "u1", "u1-pass").await;
    let stream = server
        .on_message(subscribe("granted-client", &u1_token))
        .await
        .expect("msg:sub opens the stream");
    drop(stream);
    engine.close().await;
}

/// A bind failure must fail the engine start with its reason, not be logged
/// inside the transport task while the engine reports a clean start.
///
/// The address is TEST-NET-1 (`192.0.2.0/24`): assigned to no local
/// interface, so the bind fails on every platform. A port held by another
/// listener would not: windows lets a second listener bind the same
/// address unless the first took `SO_EXCLUSIVEADDRUSE`, and the unit test
/// cannot ask a plain `TcpListener` for that.
#[tokio::test(flavor = "multi_thread")]
async fn bind_conflict_fails_the_engine_start() {
    let port = free_port();
    let table: toml::Table =
        toml::from_str(&format!("[grpc]\nhost = \"192.0.2.1\"\nport = {port}\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let started = Engine::builder()
        .set_config(&cfg)
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await;
    let err = match started {
        Ok(_) => panic!("an unassignable address must fail the engine start"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("192.0.2.1") && err.to_string().contains(&port.to_string()),
        "the failure must name the address it could not bind: {err}"
    );
}

/// The default bind is loopback, not the wildcard: the listener is bound in
/// `start` on exactly `127.0.0.1`, so a sibling
/// loopback alias of the same port is still free. A caller on another
/// interface cannot reach the service out of the box. The wildcard itself is
/// the wrong probe: Linux refuses a wildcard bind that overlaps a listening
/// specific-address socket regardless of `SO_REUSEADDR`, so it fails with
/// "address already in use" even when the server is loopback-only.
#[tokio::test(flavor = "multi_thread")]
async fn grpc_server_defaults_to_loopback() {
    let port = free_port();
    let engine = engine_with_grpc(port).await;

    let alias = std::net::TcpListener::bind(("127.0.0.2", port));
    assert!(
        alias.is_ok(),
        "the gRPC server must hold exactly 127.0.0.1 by default — the loopback \
         alias of the port must stay free and the wildcard must not be held: {alias:?}"
    );
    drop(alias);

    engine.close().await;
}

// ── uncovered paths: error mapping, guarded streams, lifecycle faults ──

use crate::MessageClient;
use acts::{AccessControl, AclError, Principal};
use std::sync::Arc as StdArc;
use tokio::sync::mpsc;

fn wire_message(name: &str) -> Message {
    Message {
        name: name.to_string(),
        seq: "wire-seq".to_string(),
        ack: None,
        data: Some(b"{}".to_vec()),
    }
}

/// `do_action` maps each action error kind onto the matching gRPC status:
/// an unknown action is NOT_FOUND, a bad payload INVALID_ARGUMENT, and a
/// server-side failure INTERNAL. The unary and the subscription paths answer
/// a refusal through the same function.
#[tokio::test(flavor = "multi_thread")]
async fn test_action_status_maps_each_error_kind() {
    let engine = Engine::builder().disable_acl().start().await.unwrap();
    let server = GrpcServer::new(&engine);

    let cases = [
        ("no:such", None, tonic::Code::NotFound),
        (
            "model:deploy",
            Some(b"{}".to_vec()),
            tonic::Code::InvalidArgument,
        ),
        (
            "model:get",
            Some(br#"{"id":"missing-model"}"#.to_vec()),
            tonic::Code::Internal,
        ),
    ];
    for (name, data, expected) in cases {
        let status = server
            .do_action(
                Message {
                    name: name.to_string(),
                    seq: "seq-1".to_string(),
                    ack: None,
                    data,
                },
                None,
                None,
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("{name} must answer an error"));
        assert_eq!(status.code(), expected, "{name}: {status}");
    }

    // an empty request seq is not echoed as an ack
    let response = server
        .do_action(
            Message {
                name: "msg:ls".to_string(),
                seq: String::new(),
                ack: None,
                data: None,
            },
            None,
            None,
        )
        .await
        .expect("msg:ls must run")
        .into_inner();
    assert_eq!(response.ack, None);

    engine.close().await;
}

/// The guard on the response stream must end a subscription whose client
/// stopped reading (queue full) and one whose receiver is already gone, not
/// leave either queued forever.
#[tokio::test(flavor = "multi_thread")]
async fn test_message_client_closes_full_and_dropped_queues() {
    let engine = Engine::builder().start().await.unwrap();
    let chan = engine.channel_with_options(&ChannelOptions::default());
    let (tx, rx) = mpsc::channel::<Result<Message, tonic::Status>>(1);
    // one slot is already taken, so the next send finds the queue full
    tx.try_send(Ok(wire_message("first"))).unwrap();
    let client = MessageClient {
        addr: "127.0.0.1:1".to_string(),
        sender: tx,
        options: ChannelOptions::default(),
        chan: StdArc::downgrade(&chan),
    };

    // full: the channel is closed instead of growing a backlog
    client.send(Ok(wire_message("second")));

    // closed: the receiver is gone
    drop(rx);
    client.send(Ok(wire_message("third")));

    engine.close().await;
}

/// Both bearer spellings are credentials; anything else — another scheme, an
/// empty value, binary metadata — is no credential at all.
#[tokio::test(flavor = "multi_thread")]
async fn test_bearer_token_variants() {
    init_tracing();
    let engine = Engine::builder().disable_acl().start().await.unwrap();
    let server = GrpcServer::new(&engine);

    for header in ["Bearer tok-1", "bearer tok-2"] {
        let mut request = send_request("msg:ls", None);
        request
            .metadata_mut()
            .insert("authorization", header.parse().unwrap());
        assert_eq!(
            GrpcServer::bearer_token(&request).as_deref(),
            Some(header.split_once(' ').unwrap().1)
        );
        server.send(request).await.expect("the action must run");
    }

    for header in ["Basic abc", "Bearer"] {
        let mut request = send_request("msg:ls", None);
        request
            .metadata_mut()
            .insert("authorization", header.parse().unwrap());
        assert_eq!(GrpcServer::bearer_token(&request), None, "{header:?}");
        server.send(request).await.expect("the action must run");
    }

    // a request without metadata at all has no credential
    assert_eq!(GrpcServer::bearer_token(&tonic::Request::new(())), None);

    engine.close().await;
}

/// An access control whose `authenticate` always refuses: the shipped
/// registries answer an absent token with the anonymous principal, so this is
/// the only way to reach the transports' unauthenticated mapping.
#[derive(Debug)]
struct RefusingAcl;

impl RefusingAcl {
    fn failure() -> AclError {
        AclError::Unauthenticated("no such session".to_string())
    }
}

#[async_trait::async_trait]
impl AccessControl for RefusingAcl {
    async fn load(&self, _store: Arc<acts::Store>) -> acts::Result<()> {
        Ok(())
    }

    fn enabled(&self) -> bool {
        true
    }

    fn authenticate(&self, _token: Option<&str>) -> std::result::Result<Principal, AclError> {
        Err(Self::failure())
    }

    async fn login(
        &self,
        _user: &str,
        _password: &str,
    ) -> std::result::Result<acts::LoginTokens, AclError> {
        Err(Self::failure())
    }

    async fn refresh(
        &self,
        _refresh_token: &str,
    ) -> std::result::Result<acts::LoginTokens, AclError> {
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

    async fn get_user(&self, _name: &str) -> Option<serde_json::Value> {
        None
    }
}

/// A credential the ACL cannot resolve is UNAUTHENTICATED on both paths.
#[tokio::test(flavor = "multi_thread")]
async fn test_authentication_failure_maps_to_unauthenticated() {
    let engine = Engine::builder()
        .set_acl(Arc::new(RefusingAcl))
        .start()
        .await
        .unwrap();
    let server = GrpcServer::new(&engine);

    let status = server
        .send(send_request("msg:clear", Some("token")))
        .await
        .expect_err("an action must be refused without a resolvable credential");
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status}");

    let status = server
        .on_message(tonic::Request::new(MessageOptions::default()))
        .await
        .err()
        .expect("a subscription must be refused without a credential");
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status}");

    engine.close().await;
}

/// The failure of registering a bound listener names the address and the
/// underlying reason, so an operator can see which bind could not be served.
#[test]
fn test_register_listener_error_names_the_address() {
    let addr: std::net::SocketAddr = "127.0.0.1:10080".parse().unwrap();
    let err = crate::register_listener_error(addr, std::io::Error::other("no reactor"));
    let text = err.to_string();
    assert!(text.contains("127.0.0.1:10080"), "{text}");
    assert!(text.contains("no reactor"), "{text}");
}

/// The failure of configuring the bound listener names the address and the
/// underlying reason, like the registration failure beside it.
#[test]
fn test_configure_listener_error_names_the_address() {
    let addr: std::net::SocketAddr = "127.0.0.1:10080".parse().unwrap();
    let err = crate::configure_listener_error(addr, std::io::Error::other("denied"));
    let text = err.to_string();
    assert!(text.contains("127.0.0.1:10080"), "{text}");
    assert!(text.contains("denied"), "{text}");
}

/// An unparseable bind address must fail the engine start with the reason.
#[tokio::test(flavor = "multi_thread")]
async fn test_prepare_rejects_invalid_bind_address() {
    let table: toml::Table = toml::from_str("[grpc]\nhost = \"not a host\"\nport = 1\n").unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let err = match Engine::builder()
        .set_config(&cfg)
        .add_plugin(&GrpcPlugin::new())
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

/// An `on_message` RPC over the real transport carries the caller's address,
/// and the stream ends cleanly when the client drops it.
#[tokio::test(flavor = "multi_thread")]
async fn test_on_message_over_the_wire_tracks_the_remote_address() {
    let port = free_port();
    let engine = engine_with_grpc(port).await;
    let url = format!("http://127.0.0.1:{port}");
    let mut client = loop {
        match ActsServiceClient::connect(url.clone()).await {
            Ok(client) => break client,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };
    let options = MessageOptions {
        client_id: "wire-client".to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    };
    let response = client
        .on_message(options)
        .await
        .expect("the subscription opens over the wire");
    drop(response);

    engine.close().await;
}

// ---- ingress hardening (design §4.2) ----

use crate::TlsConfig;
use acts::RunningTransport as _;
use tonic::{Code, Status};

/// An engine with the `[grpc]` port pinned plus any extra config table text
/// (the `[ingress]`/`[subscriptions]` overrides the admission cases need).
async fn engine_with_table(port: u16, extra: &str) -> Engine {
    let table: toml::Table = toml::from_str(&format!("[grpc]\nport = {port}\n{extra}")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    Engine::builder()
        .set_config(&cfg)
        .disable_acl()
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .unwrap()
}

/// A wire client on `port`, retried until the server accepts connections.
async fn wire_client(port: u16) -> ActsServiceClient<tonic::transport::Channel> {
    let url = format!("http://127.0.0.1:{port}");
    loop {
        if let Ok(client) = ActsServiceClient::connect(url.clone()).await {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The status details as the external-error JSON they carry.
fn details(status: &Status) -> serde_json::Value {
    let raw = status.details();
    serde_json::from_slice(raw).unwrap_or_else(|e| {
        panic!(
            "status details must be the external-error json: {e}; {}",
            status.message()
        )
    })
}

/// A store that can hold every write behind a gate: while the gate is shut,
/// each `put` awaits until it reopens. The deadline and drain cases use it
/// to pin a dispatch mid-flight deterministically — the write-behind writer
/// awaits the backend, and the dispatch awaits the writer.
struct GateStore {
    inner: acts::MemoryStore,
    gate: tokio::sync::watch::Receiver<bool>,
}

#[async_trait::async_trait]
impl acts::KvStore for GateStore {
    async fn one(&self, key: &str) -> acts::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> acts::Result<()> {
        // Startup writes must pass: the gate only ever shuts after the
        // engine is up, and reopens before the next phase asserts.
        while !*self.gate.borrow() {
            self.gate
                .clone()
                .changed()
                .await
                .map_err(|_| acts::ActError::Store("gate dropped".to_string()))?;
        }
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> acts::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[acts::StoreBatchOp],
        guards: &[acts::StoreGuard],
    ) -> acts::Result<bool> {
        while !*self.gate.borrow() {
            self.gate
                .clone()
                .changed()
                .await
                .map_err(|_| acts::ActError::Store("gate dropped".to_string()))?;
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: acts::ScanOptions,
    ) -> acts::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

fn gated_store() -> (Arc<dyn acts::KvStore>, tokio::sync::watch::Sender<bool>) {
    let (tx, rx) = tokio::sync::watch::channel(true);
    (
        Arc::new(GateStore {
            inner: acts::MemoryStore::new(),
            gate: rx,
        }),
        tx,
    )
}

/// The `[grpc]` keepalive knobs keep their documented defaults and clamp
/// their extremes, and the `[grpc].tls` table deserializes with an optional
/// client CA.
#[test]
fn test_grpc_config_keepalive_and_tls() {
    let config: GrpcConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(
        config.http2_keepalive_secs(),
        crate::config::DEFAULT_HTTP2_KEEPALIVE_SECS
    );
    assert_eq!(
        config.http2_keepalive_timeout_secs(),
        crate::config::DEFAULT_HTTP2_KEEPALIVE_TIMEOUT_SECS
    );
    assert!(
        !config.plain_behind_proxy,
        "the plaintext-behind-proxy acknowledgement is opt-in"
    );
    assert!(config.tls.is_none(), "TLS is opt-in");

    let config: GrpcConfig = serde_json::from_value(json!({
        "http2_keepalive_secs": 1,
        "http2_keepalive_timeout_secs": 100_000,
        "plain_behind_proxy": true,
        "tls": {"cert": "/tmp/c.pem", "key": "/tmp/k.pem", "client_ca": "/tmp/ca.pem"},
    }))
    .unwrap();
    assert_eq!(
        config.http2_keepalive_secs(),
        5,
        "a ping faster than 5 s clamps to 5"
    );
    assert_eq!(
        config.http2_keepalive_timeout_secs(),
        600,
        "a ping timeout past 10 min clamps to 600"
    );
    assert!(config.plain_behind_proxy);
    assert_eq!(config.tls.as_ref().unwrap().cert, "/tmp/c.pem");
    assert_eq!(
        config.tls.as_ref().unwrap().client_ca.as_deref(),
        Some("/tmp/ca.pem")
    );

    // client_ca is optional: server-only TLS is the common shape
    let config: GrpcConfig =
        serde_json::from_value(json!({"tls": {"cert": "/tmp/c.pem", "key": "/tmp/k.pem"}}))
            .unwrap();
    assert!(config.tls.as_ref().unwrap().client_ca.is_none());
}

/// Strict mode, level-driven (§2.0): a non-loopback gRPC bind must carry
/// TLS (strict), TLS or the proxy acknowledgement (standard), or nothing in
/// particular beyond a warning (permissive). The loopback default is always
/// allowed.
#[test]
fn test_strict_bind_check() {
    use acts::Level;
    use std::net::SocketAddr;
    let remote: SocketAddr = "203.0.113.7:10080".parse().unwrap();
    let loopback: SocketAddr = "127.0.0.1:10080".parse().unwrap();
    let tls = Some(TlsConfig {
        cert: "c".to_string(),
        key: "k".to_string(),
        client_ca: None,
    });

    // standard (the default): refused without TLS or the acknowledgement
    let err = GrpcPlugin::strict_bind_check(remote, &None, false, Level::Standard)
        .expect_err("a remote plaintext bind without the proxy acknowledgement refuses");
    assert!(err.to_string().contains("plain_behind_proxy"), "{err}");
    GrpcPlugin::strict_bind_check(remote, &tls, false, Level::Standard)
        .expect("a remote bind with TLS configured is accepted");
    GrpcPlugin::strict_bind_check(remote, &None, true, Level::Standard)
        .expect("a remote bind behind an acknowledged proxy is accepted");

    // permissive: warns and starts anyway
    GrpcPlugin::strict_bind_check(remote, &None, false, Level::Permissive)
        .expect("permissive starts a remote plaintext bind with a warning");

    // strict: only TLS rescues the bind, never the proxy acknowledgement
    let err = GrpcPlugin::strict_bind_check(remote, &None, true, Level::Strict)
        .expect_err("strict refuses a remote plaintext bind even behind a proxy");
    assert!(err.to_string().contains("strict"), "{err}");
    GrpcPlugin::strict_bind_check(remote, &tls, false, Level::Strict)
        .expect("strict accepts a remote bind with TLS");

    // the loopback default is allowed at every level
    for level in [Level::Permissive, Level::Standard, Level::Strict] {
        GrpcPlugin::strict_bind_check(loopback, &None, false, level)
            .expect("the loopback default is always allowed");
    }
}

/// The section's own `level` key wins over `[security].level`, and the
/// start-time refusal for a non-loopback plaintext bind follows it.
#[tokio::test(flavor = "multi_thread")]
async fn test_level_drives_the_start_refusal() {
    let port = free_port();

    // standard (inherited): the wildcard bind refuses the start
    let table: toml::Table =
        toml::from_str(&format!("[grpc]\nport = {port}\nhost = \"0.0.0.0\"\n")).unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let err = Engine::builder()
        .set_config(&cfg)
        .disable_acl()
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .map(|_| ())
        .expect_err("a wildcard plaintext bind refuses under the default level");
    assert!(err.to_string().contains("plain_behind_proxy"), "{err}");

    // strict via [security]: the same bind refuses with the strict wording
    let table: toml::Table = toml::from_str(&format!(
        "[security]\nlevel = \"strict\"\n\n[grpc]\nport = {}\nhost = \"0.0.0.0\"\nplain_behind_proxy = true\n",
        free_port()
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let err = Engine::builder()
        .set_config(&cfg)
        .disable_acl()
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .map(|_| ())
        .expect_err("strict refuses a plaintext wildcard bind despite the proxy flag");
    assert!(err.to_string().contains("strict"), "{err}");

    // permissive: the same bind starts (warned, not refused)
    let table: toml::Table = toml::from_str(&format!(
        "[grpc]\nport = {port}\nhost = \"0.0.0.0\"\nlevel = \"permissive\"\n"
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let engine = Engine::builder()
        .set_config(&cfg)
        .disable_acl()
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .expect("permissive starts a wildcard plaintext bind with a warning");
    engine.close().await;
}

/// A message past the 8 MiB decode cap is refused by the transport with
/// RESOURCE_EXHAUSTED and never reaches a dispatch (no request metric for
/// the action).
#[tokio::test(flavor = "multi_thread")]
async fn test_oversize_message_is_resource_exhausted() {
    let port = free_port();
    let engine = engine_with_table(port, "").await;
    let mut client = wire_client(port).await;

    let message = Message {
        name: "snap:upsert".to_string(),
        seq: "seq-1".to_string(),
        ack: None,
        data: Some(vec![b' '; 9 * 1024 * 1024]),
    };
    let status = client
        .send(tonic::Request::new(message))
        .await
        .expect_err("an oversize message must be refused");
    // The decode cap is enforced by the generated server before prost
    // allocates the message; tonic answers its own `OutOfRange` for a length
    // past `max_decoding_message_size` (the action budget's
    // `resource_exhausted` is a different, later refusal).
    assert_eq!(
        status.code(),
        Code::OutOfRange,
        "the server-level decode cap answers before the handler: {status}"
    );
    assert!(
        status.message().contains("limit"),
        "the refusal names the limit: {status}"
    );

    // the refusal happened before admission: no request metric exists for
    // the action, so no dispatch was ever started
    let snapshot = engine.ingress().metrics.snapshot();
    assert!(
        !snapshot.keys().any(|k| k.contains("action=snap:upsert")),
        "an oversize refusal must not count a dispatch: {snapshot:?}"
    );
    engine.close().await;
}

/// Wire fields are bounded before anything is allocated from them: an
/// oversized `name`, `client_id` or options map answers invalid_argument.
#[tokio::test(flavor = "multi_thread")]
async fn test_field_bounds_are_invalid_argument() {
    let port = free_port();
    let engine = engine_with_table(port, "").await;
    let mut client = wire_client(port).await;

    // a 300-byte action name over the wire
    let status = client
        .send(action(&"n".repeat(300), json!({})))
        .await
        .expect_err("an oversized name must be refused");
    assert_eq!(status.code(), Code::InvalidArgument, "{status}");
    let body = details(&status);
    assert_eq!(body["code"], "invalid_argument");
    assert!(body["correlation_id"].as_str().is_some(), "{body}");

    // an oversized `ack` on an otherwise valid request
    let status = client
        .send(tonic::Request::new(Message {
            name: "snap:upsert".to_string(),
            seq: "seq-1".to_string(),
            ack: Some("a".repeat(1025)),
            data: Some(b"{}".to_vec()),
        }))
        .await
        .expect_err("an oversized ack must be refused");
    assert_eq!(status.code(), Code::InvalidArgument, "{status}");

    engine.close().await;
}

/// The subscription request's own fields are bounded before the channel
/// allocation: an oversized client id or a too-large options map answers
/// invalid_argument.
#[tokio::test(flavor = "multi_thread")]
async fn test_subscription_field_bounds() {
    let engine = engine_with_grpc(free_port()).await;
    let server = GrpcServer::new(&engine);

    let oversized_client = MessageOptions {
        client_id: "c".repeat(300),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    };
    let status = server
        .on_message(tonic::Request::new(oversized_client))
        .await
        .err()
        .unwrap_or_else(|| panic!("an oversized client_id must be refused"));
    assert_eq!(status.code(), Code::InvalidArgument, "{status}");

    // 65 options entries past the 64 cap
    let many_options: std::collections::HashMap<String, String> = (0..65)
        .map(|i| (format!("k{i}"), "v".to_string()))
        .collect();
    let oversized_map = MessageOptions {
        client_id: "bounded".to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: many_options,
    };
    let status = server
        .on_message(tonic::Request::new(oversized_map))
        .await
        .err()
        .unwrap_or_else(|| panic!("a 65-entry options map must be refused"));
    assert_eq!(status.code(), Code::InvalidArgument, "{status}");

    // a value past its 1 KiB cap
    let big_value = MessageOptions {
        client_id: "bounded".to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: [("k".to_string(), "v".repeat(1025))].into_iter().collect(),
    };
    let status = server
        .on_message(tonic::Request::new(big_value))
        .await
        .err()
        .unwrap_or_else(|| panic!("an oversized option value must be refused"));
    assert_eq!(status.code(), Code::InvalidArgument, "{status}");

    engine.close().await;
}

/// Authentication happens before the payload decode: an invalid credential
/// with a malformed payload answers UNAUTHENTICATED, not invalid_argument —
/// the order the audit finding demanded.
#[tokio::test(flavor = "multi_thread")]
async fn test_auth_precedes_the_payload_decode() {
    let engine = engine_with_users(&[("alice", "wonderland", &["snap:upsert"])]).await;
    let server = GrpcServer::new(&engine);
    let token = login(&engine, "alice", "wonderland").await;
    let _ = token;

    let message = Message {
        name: "snap:upsert".to_string(),
        seq: "seq-1".to_string(),
        ack: None,
        data: Some(b"{ malformed".to_vec()),
    };
    let status = server
        .do_action(message, Some("stale-or-forged".to_string()), None)
        .await
        .expect_err("an invalid credential must be refused");
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "auth must run before the decode: {status}"
    );
    engine.close().await;
}

/// Under parse pressure (every parse permit held) a send answers
/// resource_exhausted with the `busy` code and counts the reason.
#[tokio::test(flavor = "multi_thread")]
async fn test_parse_pressure_is_busy() {
    let port = free_port();
    let engine = engine_with_table(port, "").await;
    let mut client = wire_client(port).await;

    let parse_inflight = engine
        .config()
        .get::<acts::IngressConfig>("ingress")
        .unwrap_or_default()
        .parse_inflight
        .unwrap_or(128);
    let permits: Vec<_> = (0..parse_inflight)
        .map(|_| {
            engine
                .ingress()
                .try_enter_parse()
                .expect("the parse pool starts full")
        })
        .collect();
    assert_eq!(permits.len(), parse_inflight);

    let status = client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 1, "data": {}}),
        ))
        .await
        .expect_err("a send under parse pressure must be refused");
    assert_eq!(status.code(), Code::ResourceExhausted, "{status}");
    let body = details(&status);
    assert_eq!(body["code"], "resource_exhausted", "{body}");
    assert_eq!(body["retryable"], true, "{body}");

    let snapshot = engine.ingress().metrics.snapshot();
    assert!(
        snapshot
            .keys()
            .any(|k| k.starts_with("acts_ingress_rejected_total")
                && k.contains("reason=parse_pressure")),
        "the parse refusal must be counted: {snapshot:?}"
    );

    // the permits release: the same request goes through
    drop(permits);
    let answer = client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 1, "data": {}}),
        ))
        .await
        .expect("after the parse pool frees, the request passes");
    assert_eq!(payload(&answer.into_inner()), json!(true));
    engine.close().await;
}

/// The per-action inflight cap refuses a second concurrent send with
/// resource_exhausted and counts `action_inflight`.
#[tokio::test(flavor = "multi_thread")]
async fn test_action_cap_refuses_the_second_send() {
    let port = free_port();
    let engine = engine_with_table(
        port,
        "[ingress.profile.\"snap:upsert\"]\nmax_inflight = 1\n",
    )
    .await;
    let mut client = wire_client(port).await;

    let _held = engine
        .ingress()
        .try_enter_action("snap:upsert")
        .expect("the cap of one has room for the first");
    let status = client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 1, "data": {}}),
        ))
        .await
        .expect_err("the second concurrent send must be refused");
    assert_eq!(status.code(), Code::ResourceExhausted, "{status}");
    let snapshot = engine.ingress().metrics.snapshot();
    assert!(
        snapshot
            .keys()
            .any(|k| k.starts_with("acts_ingress_rejected_total")
                && k.contains("reason=action_inflight")),
        "the action-cap refusal must be counted: {snapshot:?}"
    );

    drop(_held);
    let answer = client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 1, "data": {}}),
        ))
        .await
        .expect("after the permit releases, the send passes");
    assert_eq!(payload(&answer.into_inner()), json!(true));
    engine.close().await;
}

/// The dispatch deadline: a dispatch that does not finish inside the
/// profile's budget answers `unavailable` with the `deadline_exceeded` code
/// and the reconcile-via-reads wording; a normal dispatch under the same
/// profile passes.
#[tokio::test(flavor = "multi_thread")]
async fn test_deadline_exceeded_and_normal_pass() {
    let port = free_port();
    let (store, gate) = gated_store();
    let table: toml::Table = toml::from_str(&format!(
        "[grpc]\nport = {port}\n[ingress.profile.\"acl:login\"]\ndeadline_ms = 2000\n"
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    // A user registry, because its login session write is one of the few
    // dispatch paths that awaits the store (a gated batch) instead of the
    // write-behind queue: it gives the deadline something real to cut.
    let engine = Engine::builder()
        .set_config(&cfg)
        .set_store(store)
        .with_user_acl()
        .add_plugin(&GrpcPlugin::new())
        .start()
        .await
        .unwrap();
    engine
        .acl()
        .set_user(&acts::UserSpec {
            name: "reader".to_string(),
            add_passwords: vec!["reader-pass".to_string()],
            ..Default::default()
        })
        .await
        .unwrap();
    let mut client = wire_client(port).await;

    let login = |client: &mut ActsServiceClient<tonic::transport::Channel>| {
        let mut client = client.clone();
        async move {
            client
                .send(action(
                    "acl:login",
                    json!({"user": "reader", "password": "reader-pass"}),
                ))
                .await
        }
    };

    // a plain login under the same profile passes
    login(&mut client)
        .await
        .expect("a fast login passes the deadline");

    // shut the store gate: the session write can never finish, so the
    // server-side deadline must elapse first
    gate.send(false).unwrap();
    let status = login(&mut client)
        .await
        .expect_err("a dispatch past the deadline must answer unavailable");
    assert_eq!(status.code(), Code::Unavailable, "{status}");
    let body = details(&status);
    assert_eq!(body["code"], "unavailable", "{body}");
    assert!(
        status.message().contains("deadline_exceeded"),
        "the wording tells the caller to reconcile, not retry: {status}"
    );
    gate.send(true).unwrap();
    engine.close().await;
}

/// The client's own gRPC deadline is honored when it is tighter than the
/// server's profile deadline: the request metadata maps onto a duration and
/// the dispatch budget takes the minimum. A request with a generous client
/// timeout passes normally.
#[tokio::test(flavor = "multi_thread")]
async fn test_client_timeout_is_honored() {
    // the grpc-timeout metadata grammar: value + unit
    assert_eq!(
        crate::parse_grpc_timeout("500m"),
        Some(Duration::from_millis(500))
    );
    assert_eq!(
        crate::parse_grpc_timeout("2S"),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        crate::parse_grpc_timeout("1H"),
        Some(Duration::from_secs(3600))
    );
    assert_eq!(
        crate::parse_grpc_timeout("10u"),
        Some(Duration::from_micros(10))
    );
    assert_eq!(
        crate::parse_grpc_timeout("10n"),
        Some(Duration::from_nanos(10))
    );
    assert_eq!(
        crate::parse_grpc_timeout("2M"),
        Some(Duration::from_secs(120))
    );
    assert_eq!(crate::parse_grpc_timeout("x"), None);
    assert_eq!(crate::parse_grpc_timeout("1Q"), None);

    let port = free_port();
    let engine = engine_with_table(
        port,
        "[ingress.profile.\"snap:upsert\"]\ndeadline_ms = 1000\n",
    )
    .await;
    let mut client = wire_client(port).await;

    // a client deadline of 100 ms under a 1 s profile: the fast write passes
    let mut request = action(
        "snap:upsert",
        json!({"name": "profile", "scope": "u1", "rev": 3, "data": {}}),
    );
    request
        .metadata_mut()
        .insert("grpc-timeout", "100m".parse().unwrap());
    let answer = client
        .send(request)
        .await
        .expect("a fast dispatch passes the tighter client deadline");
    assert_eq!(payload(&answer.into_inner()), json!(true));
    engine.close().await;
}

/// Subscription quotas: the same client id cannot open a second stream on
/// the subject (`client_busy`), a saturated per-subject cap refuses further
/// subscribers (`subject_full`), and a dropped stream frees its slots so the
/// same client subscribes again.
#[tokio::test(flavor = "multi_thread")]
async fn test_subscription_quotas() {
    let port = free_port();
    let engine = engine_with_table(port, "[subscriptions]\nper_subject = 2\n").await;
    let mut client = wire_client(port).await;

    let options = |client_id: &str| MessageOptions {
        client_id: client_id.to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    };

    let first = client
        .on_message(options("dup"))
        .await
        .expect("the first opens");
    // the same client id on the same subject cannot hold a second stream
    let second = client
        .on_message(options("dup"))
        .await
        .expect_err("the same client id cannot double-subscribe");
    assert_eq!(second.code(), Code::ResourceExhausted, "{second}");
    assert!(
        second.message().contains("client_busy"),
        "{}",
        second.message()
    );

    // a second live stream (different client id) fills the per-subject cap
    let other = client
        .on_message(options("other"))
        .await
        .expect("a second client opens the subject's last slot");
    wait_until(
        || engine.ingress().subscriptions.len() == 2,
        "two live subscription leases",
    )
    .await;

    let third = client
        .on_message(options("third"))
        .await
        .expect_err("a saturated subject refuses further subscribers");
    assert_eq!(third.code(), Code::ResourceExhausted, "{third}");
    assert!(
        third.message().contains("subject_full"),
        "{}",
        third.message()
    );

    // dropping both live streams releases every bucket, and the same client
    // id can subscribe again
    drop(first);
    drop(other);
    wait_until(
        || engine.ingress().subscriptions.is_empty(),
        "the dropped streams to release their leases",
    )
    .await;
    let resumed = client
        .on_message(options("dup"))
        .await
        .expect("a dropped stream frees its lease");
    drop(resumed);

    let snapshot = engine.ingress().metrics.snapshot();
    assert!(
        snapshot
            .keys()
            .any(|k| k.starts_with("acts_ingress_rejected_total")
                && k.contains("reason=client_busy")),
        "the client_busy refusal must be counted: {snapshot:?}"
    );
    assert!(
        snapshot
            .keys()
            .any(|k| k.starts_with("acts_ingress_rejected_total")
                && k.contains("reason=subject_full")),
        "the subject_full refusal must be counted: {snapshot:?}"
    );
    engine.close().await;
}

/// Strict mode at start: a loopback bind without TLS starts; the refusal of
/// the non-loopback case is asserted by `test_strict_bind_check` (the check
/// runs before the bind, so the error path is the same one).
#[tokio::test(flavor = "multi_thread")]
async fn test_loopback_plain_start_succeeds() {
    let port = free_port();
    let engine = engine_with_table(port, "").await;
    // the transport came up: a request over the wire passes
    let mut client = wire_client(port).await;
    let answer = client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 1, "data": {}}),
        ))
        .await
        .expect("the plain loopback transport serves");
    assert_eq!(payload(&answer.into_inner()), json!(true));
    engine.close().await;
}

/// After quiesce, a send answers UNAVAILABLE and the drain report reflects
/// the unary dispatch still running; once it finishes, the drain completes.
#[tokio::test(flavor = "multi_thread")]
async fn test_quiesce_refuses_and_drain_reports_the_hanging_unary() {
    let port = free_port();
    let (store, gate) = gated_store();
    let plugin = GrpcPlugin::new();
    let table: toml::Table = toml::from_str(&format!(
        "[grpc]\nport = {port}\n[ingress.profile.\"acl:login\"]\ndeadline_ms = 30000\n"
    ))
    .unwrap();
    let cfg = acts::Config {
        data: Default::default(),
        table,
    };
    let engine = Engine::builder()
        .set_config(&cfg)
        .set_store(store)
        .with_user_acl()
        .add_plugin(&plugin)
        .start()
        .await
        .unwrap();
    engine
        .acl()
        .set_user(&acts::UserSpec {
            name: "reader".to_string(),
            add_passwords: vec!["reader-pass".to_string()],
            ..Default::default()
        })
        .await
        .unwrap();

    // a unary stuck on the gated session write: the drain must see it
    let mut client = wire_client(port).await;
    gate.send(false).unwrap();
    let unary = tokio::spawn(async move {
        client
            .send(action(
                "acl:login",
                json!({"user": "reader", "password": "reader-pass"}),
            ))
            .await
            .map(|response| response.into_inner())
    });
    wait_until(
        || plugin.transport_handle().inflight_active() == 1,
        "the unary dispatch to be admitted",
    )
    .await;

    plugin.transport_handle().quiesce().await;
    // a new dispatch through the *shared* server (same counter the transport
    // drains) is refused instead of admitted
    let status = plugin
        .server_for_test(&engine)
        .do_action(wire_message("snap:upsert"), None, None)
        .await
        .expect_err("a quiesced transport refuses new work");
    assert_eq!(status.code(), Code::Unavailable, "{status}");

    // the report names the unary still running
    let report = plugin
        .transport_handle()
        .drain(Duration::from_millis(200))
        .await;
    assert!(!report.drained, "the hanging unary must hold the drain");
    assert_eq!(report.active_after, 1);

    // releasing the store lets the admitted unary finish; the drain then
    // completes
    gate.send(true).unwrap();
    let answer = unary.await.unwrap().expect("the admitted unary completes");
    assert_eq!(answer.name, "acl:login");
    let report = plugin
        .transport_handle()
        .drain(Duration::from_secs(5))
        .await;
    assert!(
        report.drained,
        "the finished unary must not hold the drain: {report:?}"
    );
    assert_eq!(report.active_after, 0);
    engine.close().await;
}

/// The request metrics exist after calls: every unary send is counted with
/// the transport, action and result labels.
#[tokio::test(flavor = "multi_thread")]
async fn test_request_metrics_are_counted() {
    let port = free_port();
    let engine = engine_with_table(port, "").await;
    let mut client = wire_client(port).await;

    client
        .send(action(
            "snap:upsert",
            json!({"name": "profile", "scope": "u1", "rev": 1, "data": {}}),
        ))
        .await
        .expect("the ok request passes");
    let _ = client
        .send(action("snap:upsert", json!({"rev": "missing-fields"})))
        .await;

    let snapshot = engine.ingress().metrics.snapshot();
    let ok = snapshot
        .iter()
        .find(|(k, _)| {
            k.starts_with("acts_ingress_requests_total")
                && k.contains("transport=grpc")
                && k.contains("action=snap:upsert")
                && k.contains("result=ok")
        })
        .map(|(_, v)| *v)
        .unwrap_or(0);
    assert!(ok >= 1, "the ok send must be counted: {snapshot:?}");
    let error = snapshot
        .iter()
        .find(|(k, _)| {
            k.starts_with("acts_ingress_requests_total")
                && k.contains("transport=grpc")
                && k.contains("action=snap:upsert")
                && k.contains("result=error")
        })
        .map(|(_, v)| *v)
        .unwrap_or(0);
    assert!(error >= 1, "the failing send must be counted: {snapshot:?}");
    engine.close().await;
}

// ── TLS loading, strict-mode verdicts, field bounds ──

use crate::validate_message_options;
use std::path::Path;

/// The TLS loader: a valid pair builds an acceptor; every failure names the
/// file it could not use (missing, empty, malformed).
#[tokio::test(flavor = "multi_thread")]
async fn tls_acceptor_loads_a_pair_and_names_its_failures() {
    let dir = std::env::temp_dir().join(format!("acts-grpc-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cert_src = include_str!("../../acts-plugin-web/tests/test_cert.pem");
    let key_src = include_str!("../../acts-plugin-web/tests/test_key.pem");
    let cert = dir.join("c.pem");
    let key = dir.join("k.pem");
    std::fs::write(&cert, cert_src).unwrap();
    std::fs::write(&key, key_src).unwrap();
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, "").unwrap();

    let engine_with = |cert: &Path, key: &Path, client_ca: Option<&Path>| {
        let ca = client_ca
            .map(|ca| format!("\nclient_ca = {ca:?}"))
            .unwrap_or_default();
        let table: toml::Table = toml::from_str(&format!(
            "[grpc]\nport = {}\n[grpc.tls]\ncert = {cert:?}\nkey = {key:?}\n{ca}",
            free_port()
        ))
        .unwrap();
        let cfg = acts::Config {
            data: Default::default(),
            table,
        };
        Engine::builder().set_config(&cfg).start()
    };

    let engine = engine_with(&cert, &key, None).await.unwrap();
    let acceptor = GrpcPlugin::tls_acceptor(&engine).unwrap();
    assert!(acceptor.is_some(), "a matching pair builds an acceptor");
    let acceptor_err = |engine: &Engine| -> String {
        match GrpcPlugin::tls_acceptor(engine) {
            Ok(_) => panic!("the load must fail"),
            Err(err) => err.to_string(),
        }
    };
    engine.close().await;

    // a missing file is a config error naming the path
    let engine = engine_with(&dir.join("missing.pem"), &key, None)
        .await
        .unwrap();
    let err = acceptor_err(&engine);
    assert!(err.contains("missing.pem"), "{err}");
    engine.close().await;

    // an empty cert file names the file and says what is missing
    let engine = engine_with(&empty, &key, None).await.unwrap();
    let err = acceptor_err(&engine);
    assert!(err.contains("contains no certificate"), "{err}");
    engine.close().await;

    // a missing client CA is named too
    let engine = engine_with(&cert, &key, Some(&dir.join("ca-missing.pem")))
        .await
        .unwrap();
    let err = acceptor_err(&engine);
    assert!(err.contains("ca-missing.pem"), "{err}");
    engine.close().await;

    std::fs::remove_dir_all(&dir).ok();
}

/// The strict-mode verdict per posture level: permissive warns, standard
/// needs TLS or the proxy acknowledgement, strict needs TLS outright.
#[test]
fn strict_bind_check_follows_the_level() {
    use acts::Level;
    let non_loopback: std::net::SocketAddr = "0.0.0.0:10080".parse().unwrap();
    let tls = Some(TlsConfig {
        cert: "/tmp/c.pem".to_string(),
        key: "/tmp/k.pem".to_string(),
        client_ca: None,
    });

    assert!(
        GrpcPlugin::strict_bind_check(non_loopback, &None, false, Level::Permissive).is_ok(),
        "permissive only warns"
    );
    let err = GrpcPlugin::strict_bind_check(non_loopback, &None, false, Level::Standard)
        .unwrap_err()
        .to_string();
    assert!(err.contains("plain_behind_proxy"), "{err}");
    assert!(
        GrpcPlugin::strict_bind_check(non_loopback, &None, true, Level::Standard).is_ok(),
        "the acknowledgement is the standard escape hatch"
    );
    let err = GrpcPlugin::strict_bind_check(non_loopback, &None, true, Level::Strict)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not enough"), "{err}");
    assert!(
        GrpcPlugin::strict_bind_check(non_loopback, &tls, false, Level::Strict).is_ok(),
        "TLS satisfies every level"
    );
    let loopback: std::net::SocketAddr = "127.0.0.1:10080".parse().unwrap();
    assert!(GrpcPlugin::strict_bind_check(loopback, &None, false, Level::Strict).is_ok());
}

/// Subscription request bounds: the options map count, key and value sizes,
/// and the client id are all capped before allocation.
#[test]
fn subscription_field_bounds_reject_each_dimension() {
    let base = || MessageOptions {
        client_id: "c1".to_string(),
        r#type: "*".to_string(),
        state: "*".to_string(),
        uses: "*".to_string(),
        options: Default::default(),
    };

    assert!(validate_message_options(&base()).is_ok());

    let mut too_many = base();
    for i in 0..65 {
        too_many.options.insert(format!("k{i}"), "v".to_string());
    }
    assert!(validate_message_options(&too_many).is_err(), "65 options");

    let mut long_key = base();
    long_key.options.insert("k".repeat(129), "v".to_string());
    assert!(validate_message_options(&long_key).is_err(), "129-byte key");

    let mut long_value = base();
    long_value.options.insert("k".to_string(), "v".repeat(1025));
    assert!(
        validate_message_options(&long_value).is_err(),
        "1025-byte value"
    );

    let mut long_client = base();
    long_client.client_id = "c".repeat(257);
    assert!(
        validate_message_options(&long_client).is_err(),
        "257-byte client id"
    );
}

/// A unary without `data` runs with empty options instead of failing the
/// envelope: `acl:whoami` answers the anonymous identity.
#[tokio::test(flavor = "multi_thread")]
async fn a_unary_without_data_runs_with_empty_options() {
    let engine = Engine::builder().disable_acl().start().await.unwrap();
    let server = GrpcServer::new(&engine);
    let answer = server
        .do_action(
            Message {
                name: "acl:whoami".to_string(),
                seq: "seq-1".to_string(),
                ack: None,
                data: None,
            },
            None,
            None,
        )
        .await
        .expect("a dataless action runs");
    let payload: serde_json::Value =
        serde_json::from_slice(answer.get_ref().data.as_deref().unwrap()).unwrap();
    // a disabled ACL resolves every caller to the unrestricted principal,
    // so the probe answers `authenticated: true` — the point here is that
    // the dataless envelope reached the action at all.
    assert_eq!(payload["authenticated"], true);
    assert_eq!(answer.get_ref().ack.as_deref(), Some("seq-1"));
    engine.close().await;
}
