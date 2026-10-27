use crate::{
    ActsChannel,
    tests::{server_addr, start_auth_server, start_server},
};
use acts::{
    ChannelOptions, Engine, MessageState, Principal, UserSpec, Vars, Workflow,
    query::{Expr, Filter, Query},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn grpc_client_connect() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", server_addr());

    let client = ActsChannel::connect(&url).await;
    assert!(client.is_ok());
    tx.send(()).unwrap();
}

/// A malformed endpoint is rejected before any transport is attempted.
#[tokio::test]
async fn grpc_client_rejects_an_invalid_endpoint() {
    let err = ActsChannel::connect("not a valid uri").await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

/// A port nothing listens on surfaces the refused connection to the caller.
#[tokio::test]
async fn grpc_client_reports_a_refused_connection() {
    let port = super::free_port();
    let url = format!("http://{}:{port}", server_addr());
    let err = ActsChannel::connect(&url).await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

/// The snapshot feed: an upsert lands under its scope and revision, and a
/// remove tombstones it.
#[tokio::test]
async fn snapshots_upsert_and_remove() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", server_addr());
    let mut client = ActsChannel::connect(&url).await.unwrap();

    let ret = client
        .upsert_snapshot("profile", "u1", 5, crate::Vars::new().with("val", "x"))
        .await
        .unwrap();
    assert_eq!(ret.data, Some(true));

    let entry: serde_json::Value = client
        .send(
            "snap:get",
            crate::Vars::new()
                .with("name", "profile")
                .with("scope", "u1"),
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(entry["scope"], serde_json::json!("u1"));
    assert_eq!(entry["rev"], serde_json::json!(5));
    assert_eq!(entry["data"]["val"], serde_json::json!("x"));

    // a revision not newer than the cached one is ignored
    let ret = client
        .upsert_snapshot(
            "profile",
            "u1",
            5,
            crate::Vars::new().with("val", "ignored"),
        )
        .await
        .unwrap();
    assert_eq!(ret.data, Some(true));

    let ret = client.remove_snapshot("profile", "u1").await.unwrap();
    assert_eq!(ret.data, Some(true));
    let entry: serde_json::Value = client
        .send(
            "snap:get",
            crate::Vars::new()
                .with("name", "profile")
                .with("scope", "u1"),
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(entry, serde_json::Value::Null);

    // removing a target the server does not know is a refusal, not a panic
    assert!(client.remove_snapshot("unknown", "u1").await.is_err());
    tx.send(()).unwrap();
}

/// Every action helper reaches the server; on an unknown task the server's
/// refusal comes back as a status (the name mapping is the client's contract).
#[tokio::test]
async fn act_helpers_send_their_actions() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", server_addr());
    let mut client = ActsChannel::connect(&url).await.unwrap();
    let data = crate::Vars::new().with("result", "ok");

    // ack on an unknown message id
    let _ = client.ack("missing-message").await;

    // none of these panics; each answers or refuses through the transport
    let mut results = Vec::new();
    results.push(
        client
            .complete::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .submit::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .fail::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .back::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .cancel::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .skip::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .abort::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .push::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    results.push(
        client
            .remove::<serde_json::Value>("pid", "tid", data.clone())
            .await,
    );
    for result in results {
        // an `ActionResult` or a well-formed gRPC status: never an invalid
        // payload or an unimplemented action
        if let Err(status) = result {
            // the action reached the server: an unknown task is the engine's
            // answer, never an unimplemented transport path
            assert_ne!(status.code(), tonic::Code::Unimplemented);
        }
    }

    assert_eq!(client.url(), url);
    assert!(client.session().is_none());
    assert!(client.token().is_none());
    // anonymous channels have nothing to rotate or revoke
    assert_eq!(
        client.refresh().await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    assert!(!client.logout().await.unwrap());
    tx.send(()).unwrap();
}

// ---------------------------------------------------------------------------
// Assignment end to end: real gRPC server, authenticated users, engine state
// ---------------------------------------------------------------------------

/// One assigned human act owned by `alice`.
fn assigned_workflow(id: &str) -> Workflow {
    Workflow::new().with_id(id).with_step(|step| {
        step.with_id("s1")
            .with_uses("acts.core.irq", Vars::new().with("key", "approve"))
            .with_assign("alice")
    })
}

/// Declare one enabled user with a password and an allow list.
async fn add_user(engine: &Engine, name: &str, password: &str, allow: &[&str]) {
    engine
        .acl()
        .set_user(&UserSpec {
            name: name.to_string(),
            add_passwords: vec![password.to_string()],
            allow: Some(allow.iter().map(|a| (*a).to_string()).collect()),
            patterns: Some(vec!["*".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
}

/// Deploy and start `workflow` as the unrestricted embedder, answering the
/// run's pid.
async fn start_workflow(engine: &Engine, workflow: &Workflow) -> String {
    let executor = engine.executor(&Principal::unrestricted());
    executor.model().deploy(workflow, None).await.unwrap();
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap()
        .pid()
        .to_string()
}

/// Subscribe an ack channel that keeps every delivered message in memory.
fn collect_messages(engine: &Engine) -> Arc<Mutex<Vec<acts::Message>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |message| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(message.inner().clone());
        }
    });
    seen
}

/// Wait for one collected message of `nid` in `state`.
async fn wait_message(
    seen: &Mutex<Vec<acts::Message>>,
    nid: &str,
    state: MessageState,
) -> acts::Message {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(message) = seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message.nid == nid && message.state == state)
        {
            return message.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {nid} to announce {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait for one collected `type=assignment` notification at `rev`.
async fn wait_assignment(seen: &Mutex<Vec<acts::Message>>, rev: u64) -> acts::Message {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(message) = seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message.r#type == "assignment" && message.rev == rev)
        {
            return message.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the assignment notification at rev {rev}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The stored delivery status of `msg_id`, once the read model has the row.
async fn wait_delivery_status(engine: &Engine, tid: &str, msg_id: &str, expected: &str) -> String {
    let executor = engine.executor(&Principal::unrestricted());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let page = executor
            .msg()
            .list(
                &Query::new()
                    .filter(Filter::and().expr(Expr::eq("tid", tid.to_string())))
                    .limit(100),
            )
            .await
            .unwrap();
        if let Some(status) = page
            .rows
            .iter()
            .find(|row| row.msg_id == msg_id)
            .map(|row| row.status.clone())
        {
            if status == expected {
                return status;
            }
            assert_ne!(
                status, "superseded",
                "the delivery of '{msg_id}' was superseded while waiting for '{expected}'"
            );
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the delivery of '{msg_id}' to reach '{expected}'"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The stored delivery status of `msg_id`, once the read model has the row,
/// whatever that status is.
async fn wait_delivery_visible(engine: &Engine, tid: &str, msg_id: &str) -> String {
    let executor = engine.executor(&Principal::unrestricted());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let page = executor
            .msg()
            .list(
                &Query::new()
                    .filter(Filter::and().expr(Expr::eq("tid", tid.to_string())))
                    .limit(100),
            )
            .await
            .unwrap();
        if let Some(status) = page
            .rows
            .iter()
            .find(|row| row.msg_id == msg_id)
            .map(|row| row.status.clone())
        {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the delivery of '{msg_id}'"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// End to end: alice transfers the act to bob; the execution is unchanged,
/// bob decides, the old delivery is superseded, and the Completed record names
/// the new owner.
#[tokio::test(flavor = "multi_thread")]
async fn transfer_to_bob_end_to_end() {
    let (tx, rx) = oneshot::channel();
    let (engine, port) = start_auth_server(rx).await;
    add_user(&engine, "alice", "alice-pass", &["act:transfer"]).await;
    add_user(&engine, "bob", "bob-pass", &["act:complete"]).await;
    let url = format!("http://{}:{port}", server_addr());
    let mut alice = ActsChannel::connect_with_password(&url, "alice", "alice-pass")
        .await
        .unwrap();
    let mut bob = ActsChannel::connect_with_password(&url, "bob", "bob-pass")
        .await
        .unwrap();

    let seen = collect_messages(&engine);
    let workflow = assigned_workflow("e2e-transfer");
    let pid = start_workflow(&engine, &workflow).await;
    let created = wait_message(&seen, "s1", MessageState::Created).await;
    let (tid, created_id) = (created.tid.clone(), created.id.clone());

    let executor = engine.executor(&Principal::unrestricted());
    let before = executor.task().get(&pid, &tid).await.unwrap();
    let task_count = executor
        .task()
        .list(
            &Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .limit(100),
        )
        .await
        .unwrap()
        .count;

    alice
        .transfer::<serde_json::Value>(&pid, &tid, "bob", Some(0), Some("holiday"))
        .await
        .expect("alice may transfer her own act");

    let notification = wait_assignment(&seen, 1).await;
    assert_eq!(notification.owner.as_deref(), Some("bob"));
    assert_eq!(notification.assignee.as_deref(), Some("bob"));
    assert_eq!(
        notification.ref_msg_id.as_deref(),
        Some(created_id.as_str())
    );

    // The act itself is untouched: same instance, same state, no new task.
    let after = executor.task().get(&pid, &tid).await.unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.state, before.state);
    assert_eq!(
        executor
            .task()
            .list(
                &Query::new()
                    .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                    .limit(100),
            )
            .await
            .unwrap()
            .count,
        task_count,
        "a transfer creates no task"
    );

    bob.complete::<serde_json::Value>(&pid, &tid, crate::Vars::new().with("rev", 1))
        .await
        .expect("bob owns the transferred act");
    let completed = wait_message(&seen, "s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("bob"));
    assert_eq!(completed.assignee.as_deref(), Some("bob"));
    assert_eq!(completed.delegator, None);
    assert_eq!(completed.delegatee, None);
    assert_eq!(completed.rev, 1);

    wait_delivery_status(&engine, &tid, &created_id, "superseded").await;
    engine.close().await;
    tx.send(()).unwrap();
}

/// End to end: alice delegates to bob; the act keeps its owner, bob decides,
/// the old delivery is not superseded, and the Completed record names the
/// delegator and delegatee.
#[tokio::test(flavor = "multi_thread")]
async fn delegate_to_bob_end_to_end() {
    let (tx, rx) = oneshot::channel();
    let (engine, port) = start_auth_server(rx).await;
    add_user(&engine, "alice", "alice-pass", &["act:delegate"]).await;
    add_user(&engine, "bob", "bob-pass", &["act:complete"]).await;
    let url = format!("http://{}:{port}", server_addr());
    let mut alice = ActsChannel::connect_with_password(&url, "alice", "alice-pass")
        .await
        .unwrap();
    let mut bob = ActsChannel::connect_with_password(&url, "bob", "bob-pass")
        .await
        .unwrap();

    let seen = collect_messages(&engine);
    let workflow = assigned_workflow("e2e-delegate");
    let pid = start_workflow(&engine, &workflow).await;
    let created = wait_message(&seen, "s1", MessageState::Created).await;
    let (tid, created_id) = (created.tid.clone(), created.id.clone());

    let executor = engine.executor(&Principal::unrestricted());
    let before = executor.task().get(&pid, &tid).await.unwrap();
    let task_count = executor
        .task()
        .list(
            &Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .limit(100),
        )
        .await
        .unwrap()
        .count;
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 3_600_000;

    alice
        .delegate::<serde_json::Value>(&pid, &tid, "bob", Some(deadline), Some("leave"))
        .await
        .expect("alice may delegate her own act");

    let notification = wait_assignment(&seen, 1).await;
    assert_eq!(notification.owner.as_deref(), Some("alice"));
    assert_eq!(notification.assignee.as_deref(), Some("bob"));
    assert_eq!(notification.delegator.as_deref(), Some("alice"));
    assert_eq!(notification.delegatee.as_deref(), Some("bob"));
    assert_eq!(
        notification.ref_msg_id.as_deref(),
        Some(created_id.as_str())
    );

    // The delegation keeps the act: same instance, same state, no new task.
    let after = executor.task().get(&pid, &tid).await.unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.state, before.state);
    assert_eq!(
        executor
            .task()
            .list(
                &Query::new()
                    .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                    .limit(100),
            )
            .await
            .unwrap()
            .count,
        task_count,
        "a delegation creates no task"
    );

    bob.complete::<serde_json::Value>(&pid, &tid, crate::Vars::new().with("rev", 1))
        .await
        .expect("bob is the active delegatee");
    let completed = wait_message(&seen, "s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("alice"));
    assert_eq!(completed.assignee.as_deref(), Some("bob"));
    assert_eq!(completed.delegator.as_deref(), Some("alice"));
    assert_eq!(completed.delegatee.as_deref(), Some("bob"));
    assert_eq!(completed.rev, 1);

    // The owner's original delivery is kept (only a transfer supersedes it).
    let status = wait_delivery_visible(&engine, &tid, &created_id).await;
    assert_ne!(status, "superseded");
    engine.close().await;
    tx.send(()).unwrap();
}
