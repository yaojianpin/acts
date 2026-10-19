use std::{pin::Pin, time::Duration};

use futures::Stream;
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use tonic::{Code, Response, Status};

use crate::{
    ActsChannel, ActsOptions, Message, MessageOptions, SubscriptionError, Vars,
    acts_service_server::ActsService,
    model,
    tests::{serve_service, server_addr},
};

type MessageStream = Pin<Box<dyn Stream<Item = Result<Message, Status>> + Send + 'static>>;

/// A mock acts service: `Send` answers with a fixed body (or fails), and
/// `OnMessage` replays a scripted sequence of stream items.
#[derive(Default)]
struct MockService {
    /// body every `Send` answers with; `None` answers with no payload at all
    send_body: Option<Vec<u8>>,
    /// when set, every `Send` fails with this status
    send_error: Option<Status>,
    /// statuses for named actions, checked before `send_error`
    errors: std::collections::HashMap<String, Status>,
    /// when set, the `OnMessage` handshake fails with this status
    subscribe_error: Option<Status>,
    /// stream items `OnMessage` replays before closing
    script: Vec<Result<Message, Status>>,
    /// when set, the handshake's options are recorded here
    seen: Option<std::sync::Arc<parking_lot::Mutex<Option<MessageOptions>>>>,
}

fn wire(seq: &str, data: &[u8]) -> Message {
    Message {
        name: "on_message".to_string(),
        seq: seq.to_string(),
        ack: None,
        data: Some(data.to_vec()),
    }
}

/// A well-formed wire payload carrying the message id `id`.
fn payload(id: &str) -> Vec<u8> {
    let message = model::Message {
        id: id.to_string(),
        ..Default::default()
    };
    serde_json::to_vec(&message).unwrap()
}

#[tonic::async_trait]
impl ActsService for MockService {
    type OnMessageStream = MessageStream;

    async fn send(&self, request: tonic::Request<Message>) -> Result<Response<Message>, Status> {
        if let Some(status) = self.errors.get(request.get_ref().name.as_str()) {
            return Err(status.clone());
        }
        if let Some(status) = &self.send_error {
            return Err(status.clone());
        }
        Ok(Response::new(Message {
            name: "answer".to_string(),
            seq: "s1".to_string(),
            ack: None,
            data: self.send_body.clone(),
        }))
    }

    async fn on_message(
        &self,
        request: tonic::Request<MessageOptions>,
    ) -> Result<Response<Self::OnMessageStream>, Status> {
        if let Some(seen) = &self.seen {
            *seen.lock() = Some(request.get_ref().clone());
        }
        if let Some(status) = &self.subscribe_error {
            return Err(status.clone());
        }
        Ok(Response::new(Box::pin(tokio_stream::iter(
            self.script.clone(),
        ))))
    }
}

/// Start a mock service and connect a client to it. The returned sender shuts
/// the server down.
async fn connect(service: MockService) -> (ActsChannel, oneshot::Sender<()>) {
    let (tx, rx) = oneshot::channel();
    let port = serve_service(service, rx).await;
    let url = format!("http://{}:{port}", server_addr());
    let client = ActsChannel::connect(&url).await.unwrap();
    (client, tx)
}

#[tokio::test]
async fn malformed_response_is_a_status_not_a_panic() {
    let (mut client, tx) = connect(MockService {
        send_body: Some(b"{ not json".to_vec()),
        ..Default::default()
    })
    .await;

    let err = client
        .send::<()>("act:complete", Vars::new())
        .await
        .expect_err("a malformed response must be an error");
    assert_eq!(err.code(), Code::Internal);
    assert!(
        err.message().contains("act:complete")
            && err.message().contains("invalid response payload"),
        "got: {}",
        err.message()
    );
    tx.send(()).unwrap();
}

#[tokio::test]
async fn absent_response_body_is_no_data() {
    let (mut client, tx) = connect(MockService::default()).await;

    let ret = client
        .send::<()>("act:complete", Vars::new())
        .await
        .unwrap();
    assert!(ret.data.is_none());
    tx.send(()).unwrap();
}

#[tokio::test]
async fn subscription_reports_decode_faults_and_stream_errors() {
    let (mut client, tx) = connect(MockService {
        script: vec![
            Ok(wire("m-bad", b"{ not json")),
            Ok(wire("m-ok", &payload("m-ok"))),
            Err(Status::unavailable("stream gone")),
        ],
        ..Default::default()
    })
    .await;

    let (fault_tx, mut faults) = mpsc::unbounded_channel();
    let (msg_tx, mut messages) = mpsc::unbounded_channel();
    let sub = client
        .subscribe(
            "c1",
            move |m| {
                msg_tx.send(m.id.clone()).ok();
            },
            move |err| {
                fault_tx.send(err).ok();
            },
            &ActsOptions {
                ack: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let fault = timeout(Duration::from_secs(5), faults.recv())
        .await
        .unwrap()
        .unwrap();
    match fault {
        SubscriptionError::Decode { seq, source } => {
            assert_eq!(seq, "m-bad");
            assert!(source.contains("invalid response payload"), "got: {source}");
        }
        other => panic!("expected a decode fault, got {other:?}"),
    }

    // the feed survives the corrupt payload and delivers the next message
    let id = timeout(Duration::from_secs(5), messages.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id, "m-ok");

    // the stream error ends the feed and reaches the caller
    let err = timeout(Duration::from_secs(5), sub.wait())
        .await
        .unwrap()
        .expect_err("a stream error must end the subscription with an error");
    assert_eq!(err.code(), Code::Unavailable);
    assert_eq!(err.message(), "stream gone");
    tx.send(()).unwrap();
}

#[tokio::test]
async fn subscription_reports_a_clean_close() {
    let (mut client, tx) = connect(MockService::default()).await;

    let sub = client
        .subscribe("c2", |_| {}, |_| {}, &ActsOptions::default())
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_secs(5), sub.wait())
            .await
            .unwrap()
            .is_ok()
    );
    tx.send(()).unwrap();
}

#[tokio::test]
async fn failed_handshake_reaches_the_caller() {
    let (mut client, tx) = connect(MockService {
        subscribe_error: Some(Status::unavailable("no server")),
        ..Default::default()
    })
    .await;

    let err = client
        .subscribe("c3", |_| {}, |_| {}, &ActsOptions::default())
        .await
        .expect_err("a failed handshake must be an error");
    assert_eq!(err.code(), Code::Unavailable);
    tx.send(()).unwrap();
}

#[tokio::test]
async fn failed_auto_ack_is_reported_and_drops_the_message() {
    let (mut client, tx) = connect(MockService {
        send_error: Some(Status::unavailable("ack failed")),
        script: vec![Ok(wire("m-1", &payload("m-1")))],
        ..Default::default()
    })
    .await;

    let (fault_tx, mut faults) = mpsc::unbounded_channel();
    let (msg_tx, mut messages) = mpsc::unbounded_channel();
    let sub = client
        .subscribe(
            "c4",
            move |m| {
                msg_tx.send(m.id.clone()).ok();
            },
            move |err| {
                fault_tx.send(err).ok();
            },
            &ActsOptions {
                ack: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let fault = timeout(Duration::from_secs(5), faults.recv())
        .await
        .unwrap()
        .unwrap();
    match fault {
        SubscriptionError::Ack { seq, source } => {
            assert_eq!(seq, "m-1");
            assert_eq!(source.code(), Code::Unavailable);
        }
        other => panic!("expected an ack fault, got {other:?}"),
    }
    // the unacked message is not handed to the message callback
    assert!(messages.try_recv().is_err());
    assert!(
        timeout(Duration::from_secs(5), sub.wait())
            .await
            .unwrap()
            .is_ok()
    );
    tx.send(()).unwrap();
}

/// A refresh that fails for a reason other than authentication keeps the
/// original refusal: the caller learns why the request was refused, not why
/// the repair failed.
#[tokio::test]
async fn refresh_failure_keeps_the_original_authentication_status() {
    let mut errors = std::collections::HashMap::new();
    errors.insert(
        "proc:ls".to_string(),
        Status::unauthenticated("token expired"),
    );
    errors.insert(
        "acl:refresh".to_string(),
        Status::internal("refresh exploded"),
    );

    let (tx, rx) = oneshot::channel();
    let port = serve_service(
        MockService {
            errors,
            ..Default::default()
        },
        rx,
    )
    .await;
    let url = format!("http://{}:{port}", server_addr());
    let session = crate::SessionTokens {
        token: "stale".to_string(),
        refresh_token: "rt".to_string(),
        expires_at: 1,
        refresh_expires_at: 2,
    };
    let mut client = ActsChannel::connect_with_session(&url, session)
        .await
        .unwrap();

    let err = client.send::<()>("proc:ls", Vars::new()).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    assert_eq!(err.message(), "token expired");
    tx.send(()).unwrap();
}

/// A successful refresh rotates the session and retries the refused request.
#[tokio::test]
async fn a_refreshed_session_retries_the_refused_request() {
    let body = serde_json::to_vec(&serde_json::json!({
        "token": "fresh",
        "refresh_token": "rt2",
        "expires_in": 60,
        "refresh_expires_in": 120,
    }))
    .unwrap();
    let mut errors = std::collections::HashMap::new();
    errors.insert(
        "proc:ls".to_string(),
        Status::unauthenticated("token expired"),
    );

    let (tx, rx) = oneshot::channel();
    let port = serve_service(
        MockService {
            send_body: Some(body),
            errors,
            ..Default::default()
        },
        rx,
    )
    .await;
    let url = format!("http://{}:{port}", server_addr());
    let session = crate::SessionTokens {
        token: "stale".to_string(),
        refresh_token: "rt".to_string(),
        expires_at: 1,
        refresh_expires_at: 2,
    };
    let mut client = ActsChannel::connect_with_session(&url, session)
        .await
        .unwrap();

    // the retry is refused too (the mock is static), but the session was
    // rotated before it
    let err = client.send::<()>("proc:ls", Vars::new()).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    let rotated = client.session().expect("the session was rotated");
    assert_eq!(rotated.token, "fresh");
    assert_eq!(rotated.refresh_token, "rt2");
    assert_eq!(client.token().as_deref(), Some("fresh"));
    tx.send(()).unwrap();
}

/// The selector options reach the server handshake intact.
#[tokio::test]
async fn subscription_forwards_its_selectors() {
    let seen = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let (tx, rx) = oneshot::channel();
    let port = serve_service(
        MockService {
            seen: Some(seen.clone()),
            ..Default::default()
        },
        rx,
    )
    .await;
    let url = format!("http://{}:{port}", server_addr());
    let mut client = ActsChannel::connect(&url).await.unwrap();

    let mut options = std::collections::HashMap::new();
    options.insert("tag".to_string(), "g1".to_string());
    let sub = client
        .subscribe(
            "client-7",
            |_| {},
            |_| {},
            &ActsOptions {
                r#type: Some("msg".to_string()),
                state: Some("created".to_string()),
                uses: Some("@act".to_string()),
                options,
                ack: Some(false),
            },
        )
        .await
        .unwrap();

    let recorded = seen.lock().clone().expect("the handshake was recorded");
    assert_eq!(recorded.client_id, "client-7");
    assert_eq!(recorded.r#type, "msg");
    assert_eq!(recorded.state, "created");
    assert_eq!(recorded.uses, "@act");
    assert_eq!(recorded.options.get("tag").map(String::as_str), Some("g1"));

    assert!(
        timeout(Duration::from_secs(5), sub.wait())
            .await
            .unwrap()
            .is_ok()
    );
    tx.send(()).unwrap();
}

/// `ack` answers whatever the server sent (the mock sends no payload).
#[tokio::test]
async fn ack_answers_the_server_value() {
    let (mut client, tx) = connect(MockService::default()).await;
    let ret = client.ack("m1").await.unwrap();
    assert!(ret.data.is_none());
    assert!(ret.cost() >= 0);
    tx.send(()).unwrap();
}

/// The default auto-ack path: a well-formed message is acked and delivered.
#[tokio::test]
async fn successful_auto_ack_delivers_the_message() {
    let (mut client, tx) = connect(MockService {
        script: vec![Ok(wire("m-1", &payload("m-1")))],
        ..Default::default()
    })
    .await;

    let (msg_tx, mut messages) = mpsc::unbounded_channel();
    let sub = client
        .subscribe(
            "c5",
            move |m| {
                msg_tx.send(m.id.clone()).ok();
            },
            |_| {},
            &ActsOptions::default(),
        )
        .await
        .unwrap();

    let id = timeout(Duration::from_secs(5), messages.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id, "m-1");
    assert!(
        timeout(Duration::from_secs(5), sub.wait())
            .await
            .unwrap()
            .is_ok()
    );
    tx.send(()).unwrap();
}
