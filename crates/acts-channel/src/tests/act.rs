use crate::{
    ActsChannel,
    tests::{server_addr, start_server},
};
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
