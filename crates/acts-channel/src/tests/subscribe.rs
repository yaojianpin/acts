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
    tests::{SERVER_ADDR, serve_service},
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
    /// when set, the `OnMessage` handshake fails with this status
    subscribe_error: Option<Status>,
    /// stream items `OnMessage` replays before closing
    script: Vec<Result<Message, Status>>,
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

    async fn send(&self, _request: tonic::Request<Message>) -> Result<Response<Message>, Status> {
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
        _request: tonic::Request<MessageOptions>,
    ) -> Result<Response<Self::OnMessageStream>, Status> {
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
    let url = format!("http://{}:{port}", SERVER_ADDR);
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
