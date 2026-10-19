//! Tests for the generated bindings and the sequence helper.
//!
//! The bindings are committed code (`proto/acts.grpc.rs`), so the tests are
//! about their observable contract: the prost messages round-trip on the wire,
//! the client builders and calls reach the transport with the right method,
//! and the server dispatches both RPCs while answering unknown paths with
//! `Unimplemented`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use prost::Message as _;

use super::*;
use acts_service_client::ActsServiceClient;
use acts_service_server::{ActsService, ActsServiceServer, SERVICE_NAME};
use tonic::codegen::http;

fn sample_message() -> Message {
    Message {
        name: "flow-1".to_owned(),
        seq: "abcdefgh".to_owned(),
        ack: Some("ack-1".to_owned()),
        data: Some(vec![0, 1, 2, 254, 255]),
    }
}

fn sample_options() -> MessageOptions {
    MessageOptions {
        client_id: "client-1".to_owned(),
        r#type: "act".to_owned(),
        state: "ready".to_owned(),
        options: HashMap::from([
            ("region".to_owned(), "eu".to_owned()),
            ("tier".to_owned(), "gold".to_owned()),
        ]),
        uses: "uses-1".to_owned(),
    }
}

#[test]
fn message_options_defaults_are_empty() {
    let options = MessageOptions::default();

    assert_eq!(options.client_id, "");
    assert_eq!(options.r#type, "");
    assert_eq!(options.state, "");
    assert!(options.options.is_empty());
    assert_eq!(options.uses, "");
    // proto3 scalar defaults are not on the wire at all.
    assert_eq!(options.encoded_len(), 0);
    assert!(options.encode_to_vec().is_empty());
    assert_eq!(MessageOptions::decode(&[] as &[u8]).unwrap(), options);
}

#[test]
fn message_options_round_trip() {
    let options = sample_options();
    let bytes = options.encode_to_vec();

    assert_eq!(bytes.len(), options.encoded_len());
    // `client_id` is field 1, length-delimited: tag 0x0a, length 8.
    assert_eq!(bytes[0], 0x0a);
    assert_eq!(bytes[1], "client-1".len() as u8);

    let decoded = MessageOptions::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, options);
    assert_eq!(
        decoded.options.get("region").map(String::as_str),
        Some("eu")
    );
    assert_eq!(
        decoded.options.get("tier").map(String::as_str),
        Some("gold")
    );
}

#[test]
fn message_options_rejects_malformed_bytes() {
    // field 1, length-delimited, but the length runs past the buffer.
    assert!(MessageOptions::decode(&[0x0a, 0x05, b'a'][..]).is_err());
    // field 1 with wire type 7 does not exist.
    assert!(MessageOptions::decode(&[0x0f][..]).is_err());
}

#[test]
fn message_defaults_have_no_optionals() {
    let message = Message::default();

    assert_eq!(message.name, "");
    assert_eq!(message.seq, "");
    assert!(message.ack.is_none());
    assert!(message.data.is_none());
    assert_eq!(message.encoded_len(), 0);
    assert!(message.encode_to_vec().is_empty());
    assert_eq!(Message::decode(&[] as &[u8]).unwrap(), message);
}

#[test]
fn message_round_trips_optional_fields() {
    let message = sample_message();
    let bytes = message.encode_to_vec();

    assert_eq!(bytes.len(), message.encoded_len());
    assert_eq!(Message::decode(bytes.as_slice()).unwrap(), message);

    // A present-but-empty optional field stays `Some`, which is how the wire
    // distinguishes it from an absent field.
    let empties = Message {
        ack: Some(String::new()),
        data: Some(Vec::new()),
        ..Message::default()
    };
    let decoded = Message::decode(empties.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.ack.as_deref(), Some(""));
    assert_eq!(decoded.data.as_deref(), Some(&[][..]));
}

#[test]
fn message_wire_format_is_stable() {
    let message = Message {
        name: "a".to_owned(),
        ..Message::default()
    };
    // field 1 (`name`), length-delimited, one byte.
    assert_eq!(message.encode_to_vec(), vec![0x0a, 0x01, b'a']);

    let with_seq = Message {
        seq: "b".to_owned(),
        ..Message::default()
    };
    // field 2 (`seq`).
    assert_eq!(with_seq.encode_to_vec(), vec![0x12, 0x01, b'b']);

    let with_ack = Message {
        ack: Some("c".to_owned()),
        ..Message::default()
    };
    // field 3 (`ack`).
    assert_eq!(with_ack.encode_to_vec(), vec![0x1a, 0x01, b'c']);

    let with_data = Message {
        data: Some(vec![7]),
        ..Message::default()
    };
    // field 4 (`data`).
    assert_eq!(with_data.encode_to_vec(), vec![0x22, 0x01, 0x07]);
}

#[test]
fn message_rejects_malformed_bytes() {
    // truncated `name`.
    assert!(Message::decode(&[0x0a, 0x05, b'a'][..]).is_err());
    // truncated `data`.
    assert!(Message::decode(&[0x22, 0x04, 0x01][..]).is_err());
    // invalid wire type.
    assert!(Message::decode(&[0x0f][..]).is_err());
}

#[test]
fn message_is_hashable_and_clonable() {
    let message = sample_message();
    let mut set = HashSet::new();
    set.insert(message.clone());

    assert!(set.contains(&message));
    assert_eq!(message.clone(), message);
    assert!(format!("{message:?}").contains("flow-1"));
}

#[test]
fn create_seq_is_eight_alphanumeric_chars() {
    let seq = create_seq();

    assert_eq!(seq.len(), 8);
    assert!(seq.chars().all(|c| c.is_ascii_alphanumeric()));

    let seqs: HashSet<String> = (0..16).map(|_| create_seq()).collect();
    assert_eq!(seqs.len(), 16);
}

/// A transport stub: records the request so a server test can replay its
/// (lazily encoded) body, and answers with an empty, successful gRPC response.
#[derive(Clone, Default)]
struct MockClient {
    captured: Arc<Mutex<Option<http::Request<tonic::body::Body>>>>,
}

impl tonic::codegen::Service<http::Request<tonic::body::Body>> for MockClient {
    type Response = http::Response<tonic::body::Body>;
    type Error = tonic::Status;
    type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let previous = self.captured.lock().replace(req);
        assert!(previous.is_none(), "the stub handles one request at a time");

        let response = http::Response::builder()
            .status(http::StatusCode::OK)
            .header(tonic::Status::GRPC_STATUS, "0")
            .header(
                http::header::CONTENT_TYPE,
                tonic::metadata::GRPC_CONTENT_TYPE,
            )
            .body(tonic::body::Body::empty())
            .unwrap();

        Box::pin(async move { Ok(response) })
    }
}

#[tokio::test]
async fn client_builders_chain_and_send_uses_the_stub() {
    let stub = MockClient::default();
    let mut client = ActsServiceClient::new(stub.clone())
        .send_compressed(tonic::codegen::CompressionEncoding::Gzip)
        .accept_compressed(tonic::codegen::CompressionEncoding::Gzip)
        .max_decoding_message_size(4 * 1024 * 1024)
        .max_encoding_message_size(4 * 1024 * 1024);

    // The stub answers with a headers-only response, so there is no message
    // body to decode; the call still travels the full generated path.
    let error = client.send(sample_message()).await.unwrap_err();
    assert_eq!(error.code(), tonic::Code::Internal);

    let captured = stub.captured.lock().take().expect("request captured");
    assert_eq!(captured.uri().path(), "/acts.grpc.ActsService/Send");
    let method = captured
        .extensions()
        .get::<tonic::codegen::GrpcMethod<'static>>()
        .expect("grpc method extension");
    assert_eq!(method.service(), "acts.grpc.ActsService");
    assert_eq!(method.method(), "Send");
}

#[tokio::test]
async fn client_connect_rejects_an_invalid_endpoint() {
    // No dialing: `Endpoint::new` rejects the URI, so this stays offline and
    // deterministic.
    assert!(ActsServiceClient::connect("not a uri").await.is_err());
}

#[tokio::test]
async fn client_with_origin_and_interceptor_can_call() {
    let mut with_origin = ActsServiceClient::with_origin(
        MockClient::default(),
        http::Uri::from_static("http://acts.local"),
    );
    assert!(with_origin.send(sample_message()).await.is_err());

    let mut intercepted =
        ActsServiceClient::with_interceptor(MockClient::default(), |req: tonic::Request<()>| {
            Ok::<_, tonic::Status>(req)
        });
    let error = intercepted.send(sample_message()).await.unwrap_err();
    assert_eq!(error.code(), tonic::Code::Internal);
}

#[tokio::test]
async fn client_streaming_call_yields_an_empty_stream() {
    let stub = MockClient::default();
    let mut client = ActsServiceClient::new(stub.clone());

    let response = client.on_message(sample_options()).await.unwrap();
    let mut stream = response.into_inner();
    assert!(stream.message().await.unwrap().is_none());

    let captured = stub.captured.lock().take().expect("request captured");
    assert_eq!(captured.uri().path(), "/acts.grpc.ActsService/OnMessage");
}

struct StubActs {
    send_hits: Arc<AtomicUsize>,
    on_message_hits: Arc<AtomicUsize>,
}

impl StubActs {
    fn new() -> Self {
        Self {
            send_hits: Arc::new(AtomicUsize::new(0)),
            on_message_hits: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[tonic::codegen::async_trait]
impl ActsService for StubActs {
    async fn send(
        &self,
        request: tonic::Request<Message>,
    ) -> std::result::Result<tonic::Response<Message>, tonic::Status> {
        self.send_hits.fetch_add(1, Ordering::SeqCst);
        let mut message = request.into_inner();
        message.ack = Some("server-ack".to_owned());
        Ok(tonic::Response::new(message))
    }

    type OnMessageStream =
        tonic::codegen::tokio_stream::Empty<std::result::Result<Message, tonic::Status>>;

    async fn on_message(
        &self,
        _request: tonic::Request<MessageOptions>,
    ) -> std::result::Result<tonic::Response<Self::OnMessageStream>, tonic::Status> {
        self.on_message_hits.fetch_add(1, Ordering::SeqCst);
        Ok(tonic::Response::new(tonic::codegen::tokio_stream::empty()))
    }
}

fn request(path: &str, body: tonic::body::Body) -> http::Request<tonic::body::Body> {
    http::Request::builder().uri(path).body(body).unwrap()
}

#[tokio::test]
async fn server_builders_and_service_name() {
    use tonic::codegen::Service as _;

    let mut server = ActsServiceServer::new(StubActs::new())
        .accept_compressed(tonic::codegen::CompressionEncoding::Gzip)
        .send_compressed(tonic::codegen::CompressionEncoding::Gzip)
        .max_decoding_message_size(1024)
        .max_encoding_message_size(2048);
    let mut cloned = server.clone();
    let _intercepted =
        ActsServiceServer::with_interceptor(StubActs::new(), |req: tonic::Request<()>| {
            Ok::<_, tonic::Status>(req)
        });
    let _from_arc = ActsServiceServer::from_arc(Arc::new(StubActs::new()));

    assert_eq!(SERVICE_NAME, "acts.grpc.ActsService");
    assert_eq!(
        <ActsServiceServer<StubActs> as tonic::server::NamedService>::NAME,
        SERVICE_NAME
    );

    // The cloned server answers even if the original is not used further.
    let response = cloned
        .call(request(
            "/acts.grpc.ActsService/Unknown",
            tonic::body::Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);

    // Dropping a request on the built server also reaches its dispatcher.
    let response = server
        .call(request(
            "/acts.grpc.ActsService/Send",
            tonic::body::Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
}

#[tokio::test]
async fn server_unknown_path_is_unimplemented() {
    use tonic::codegen::Service as _;

    let mut server = ActsServiceServer::new(StubActs::new());
    let response = server
        .call(request(
            "/acts.grpc.OtherService/Unknown",
            tonic::body::Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(tonic::Status::GRPC_STATUS)
            .and_then(|value| value.to_str().ok()),
        Some("12")
    );
    assert_eq!(
        response.headers().get(http::header::CONTENT_TYPE),
        Some(&tonic::metadata::GRPC_CONTENT_TYPE)
    );
}

#[tokio::test]
async fn server_dispatches_both_rpcs() {
    use tonic::codegen::Service as _;

    let stub = StubActs::new();
    let mut server = ActsServiceServer::new(StubActs {
        send_hits: stub.send_hits.clone(),
        on_message_hits: stub.on_message_hits.clone(),
    });

    // Reuse the client's encoded request bodies: the stub client copies the
    // requests before their lazy bodies are polled, so the server decodes the
    // bytes the client would have put on the wire.
    let send_stub = MockClient::default();
    let mut client = ActsServiceClient::new(send_stub.clone());
    assert!(client.send(sample_message()).await.is_err());
    let body = send_stub
        .captured
        .lock()
        .take()
        .expect("send request captured")
        .into_body();

    let response = server
        .call(request("/acts.grpc.ActsService/Send", body))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response.headers().get(http::header::CONTENT_TYPE),
        Some(&tonic::metadata::GRPC_CONTENT_TYPE)
    );
    assert_eq!(stub.send_hits.load(Ordering::SeqCst), 1);

    let stream_stub = MockClient::default();
    let mut client = ActsServiceClient::new(stream_stub.clone());
    assert!(client.on_message(sample_options()).await.is_ok());
    let body = stream_stub
        .captured
        .lock()
        .take()
        .expect("on_message request captured")
        .into_body();

    let response = server
        .call(request("/acts.grpc.ActsService/OnMessage", body))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(stub.on_message_hits.load(Ordering::SeqCst), 1);
}
