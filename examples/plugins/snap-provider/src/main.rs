//! A snapshot variable provider over gRPC.
//!
//! The business system registers its variable once (an operator publishes a
//! package whose `options` metadata names `{"var": "profile", …}`) and runs
//! this client next to its own services. The client logs in as the provider
//! account, subscribes to the engine's fetch requests (`type: "req"`,
//! `uses: "profile"` — set `options` filters such as `{"tenant": "t*"}` to
//! serve only the tenants it owns) and answers each one with `snap:reply`.
//!
//! The reply carries the request id (the wire `seq`, echoed in the engine's
//! response `ack`), the scope it answers for and a monotonic `rev` — the
//! engine applies it under the same revision guard as a push feed, so a late
//! or duplicated reply can never roll a scope back.
//!
//! Run against a server started by the `grpc` example (127.0.0.1:10080):
//!
//! ```text
//! ACTS_ENDPOINT=http://127.0.0.1:10080 ACTS_USER=provider ACTS_PASSWORD=provider-pass cargo run -p acts-example-plugin-snap-provider
//! ```

use acts::Message;
use acts_proto::{Message as WireMessage, MessageOptions, acts_service_client::ActsServiceClient};
use serde_json::{Value as JsonValue, json};

const TARGET: &str = "profile";

fn endpoint() -> String {
    std::env::var("ACTS_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:10080".to_string())
}

fn credentials() -> (String, String) {
    (
        std::env::var("ACTS_USER").unwrap_or_else(|_| "provider".to_string()),
        std::env::var("ACTS_PASSWORD").unwrap_or_else(|_| "provider-pass".to_string()),
    )
}

/// The provider's own data: whatever the business system would answer for
/// this scope — profile rows, security claims, anything the workflow's
/// `$profile.<key>` expressions read. The scope key arrives composed from
/// the run's sealed owner context (`tenant/unit/…`).
fn payload_for(scope: &str) -> JsonValue {
    json!({
        "uid": scope,
        "name": format!("provider-profile-{scope}"),
        "roles": ["approver"],
        "fetched_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    })
}

/// A monotonic revision for the scope: epoch millis are enough for a demo
/// provider — the engine only requires newer-than-cached.
fn revision() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
}

#[tokio::main]
async fn main() -> acts::Result<()> {
    let mut client = ActsServiceClient::connect(endpoint())
        .await
        .expect("connect to the acts server");

    // login: the provider account's snapshot scopes decide which scopes this
    // client may answer for — a reply for a scope it does not own is refused
    // by the engine's `snap:reply` gate.
    let (user, password) = credentials();
    let login = WireMessage {
        name: "acl:login".to_string(),
        seq: acts_proto::create_seq(),
        data: Some(
            json!({ "user": user, "password": password })
                .to_string()
                .into_bytes(),
        ),
        ..Default::default()
    };
    let answer = client.send(login).await.expect("login");
    let tokens: JsonValue =
        serde_json::from_slice(answer.into_inner().data.as_deref().unwrap_or(&[]))
            .expect("login answer");
    let _token = tokens["token"]
        .as_str()
        .expect("the access token")
        .to_string();
    println!("logged in as the provider");

    // subscribe to the fetch requests of one target
    let mut requests = client
        .on_message(MessageOptions {
            client_id: "snap-provider".to_string(),
            r#type: "req".to_string(),
            uses: TARGET.to_string(),
            ..Default::default()
        })
        .await
        .expect("subscribe to fetch requests")
        .into_inner();

    println!("serving fetch requests for '{TARGET}'…");
    while let Some(request) = requests.message().await.transpose() {
        let request = match request {
            Ok(request) => request,
            Err(err) => {
                eprintln!("stream error: {err}");
                break;
            }
        };
        // the wire `data` is the engine's own message: the scope and the
        // request id travel in its inputs
        let engine_msg: Message = request
            .data
            .as_deref()
            .and_then(|data| serde_json::from_slice(data).ok())
            .unwrap_or_default();
        let req_id = engine_msg
            .inputs
            .get::<String>("req_id")
            .unwrap_or_else(|| request.seq.clone());
        let scope = engine_msg.inputs.get::<String>("scope").unwrap_or_default();
        println!("fetch request {req_id} for scope '{scope}'");

        let reply = WireMessage {
            name: "snap:reply".to_string(),
            seq: request.seq.clone(),
            data: Some(
                json!({
                    "req_id": req_id,
                    "name": TARGET,
                    "scope": scope,
                    "rev": revision(),
                    "data": payload_for(&scope),
                })
                .to_string()
                .into_bytes(),
            ),
            ..Default::default()
        };
        let answer = client.send(reply).await.expect("send reply");
        // the server echoes the request `seq` in `ack`: the reply was applied
        let applied = answer.into_inner().ack.as_deref() == Some(request.seq.as_str());
        println!("reply applied: {applied}");
    }

    Ok(())
}
