//! Live NATS tests for the `acts.app.pubsub.nats` package.
//!
//! Requires a reachable NATS server: `ACTS_NATS_URL` (default
//! `nats://127.0.0.1:4222`). When no server responds the tests skip, so plain
//! `cargo test` stays green without NATS; CI runs them against a service
//! container.
//!
//! Each case is a real run through the engine — the package's `execute` is
//! reached only that way — and reads the run's terminal event.

use acts::{Config, Engine, Principal, Signal, Step, Vars, Workflow};
use acts_package_nats::NatsPackage;
use async_nats::Client;
use futures_util::StreamExt;
use serde_json::{Value as JsonValue, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn nats_url() -> String {
    std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string())
}

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

fn config(url: &str, timeout_ms: u64) -> Config {
    Config {
        data: Default::default(),
        table: toml::from_str::<toml::Table>(&format!(
            "[nats]\nurl = \"{url}\"\ntimeout-ms = {timeout_ms}\n"
        ))
        .unwrap(),
    }
}

/// A subject unique to one call: live cells share the broker.
fn subject(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("acts.pkg.nats.{tag}.{nanos}")
}

/// An engine over `url`, with the terminal events collected into one signal
/// that closes after `wanted` of them.
async fn engine(url: &str, timeout_ms: u64, wanted: usize) -> (Engine, Signal<Vec<(bool, Vars)>>) {
    let engine = Engine::builder()
        .set_config(&config(url, timeout_ms))
        .add_package::<NatsPackage>()
        .start()
        .await
        .unwrap();

    let ended = engine.signal::<Vec<(bool, Vars)>>(Vec::new());
    let error_ended = ended.clone();
    engine.channel().on_error(move |e| {
        let ended = error_ended.clone();
        let outputs = e.outputs.clone();
        async move {
            ended.update(|all| all.push((true, outputs.clone())));
            if ended.data().len() >= wanted {
                ended.close();
            }
        }
    });
    let complete_ended = ended.clone();
    engine.channel().on_complete(move |e| {
        let ended = complete_ended.clone();
        let outputs = e.outputs.clone();
        async move {
            ended.update(|all| all.push((false, outputs.clone())));
            if ended.data().len() >= wanted {
                ended.close();
            }
        }
    });
    (engine, ended)
}

fn act(id: &str, params: JsonValue) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(
        Step::new()
            .with_id("s1")
            .with_uses("acts.app.pubsub.nats", params.into()),
    );
    workflow
}

async fn start(engine: &Engine, principal: &Principal, workflow: &Workflow) {
    let executor = engine.executor(principal);
    executor.model().deploy(workflow, None).await.unwrap();
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();
}

fn data_of(outputs: &[(bool, Vars)]) -> Option<JsonValue> {
    outputs
        .iter()
        .find_map(|(_, vars)| vars.get::<JsonValue>("data"))
}

/// A JSON body published to the broker arrives as JSON in the subscribing
/// act, and a message published by the pub act arrives at the broker as the
/// JSON text of the act's `message` param.
#[tokio::test(flavor = "multi_thread")]
async fn publish_and_subscribe_round_trip_messages() {
    let Some(client) = connect_or_skip().await else {
        return;
    };
    let url = nats_url();
    let sub_subject = subject("roundtrip-sub");
    let pub_subject = subject("roundtrip-pub");

    // the raw client must be subscribed before the pub act runs
    let mut raw = client.subscribe(pub_subject.clone()).await.unwrap();

    let (engine, ended) = engine(&url, 5_000, 2).await;
    let principal = Principal::unrestricted();

    let mut sub = Workflow::new().with_id("nats_sub");
    sub.steps.push(acts::Step::new().with_id("s1").with_uses(
        "acts.app.pubsub.nats",
        json!({ "mode": "sub", "subject": sub_subject.clone() }).into(),
    ));
    start(&engine, &principal, &sub).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut publish = Workflow::new().with_id("nats_pub");
    publish
        .steps
        .push(acts::Step::new().with_id("s1").with_uses(
            "acts.app.pubsub.nats",
            json!({ "mode": "pub", "subject": pub_subject.clone(), "message": "hello" }).into(),
        ));
    start(&engine, &principal, &publish).await;

    client
        .publish(sub_subject, "{\"hello\":\"world\"}".into())
        .await
        .unwrap();
    client.flush().await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(5), raw.next())
        .await
        .expect("the broker delivers the published message")
        .expect("a message arrives");
    assert_eq!(
        String::from_utf8_lossy(&received.payload),
        "\"hello\"",
        "the pub act sends the message's JSON text"
    );

    let outputs = ended.recv().await;
    assert_eq!(outputs.len(), 2, "both runs report: {outputs:?}");
    assert!(
        outputs.iter().all(|(failed, _)| !failed),
        "neither run fails: {outputs:?}"
    );
    assert_eq!(data_of(&outputs), Some(json!({ "hello": "world" })));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_json_payload_arrives_as_a_string() {
    let Some(client) = connect_or_skip().await else {
        return;
    };
    let url = nats_url();
    let subject = subject("raw");
    let (engine, ended) = engine(&url, 5_000, 1).await;
    let principal = Principal::unrestricted();

    start(
        &engine,
        &principal,
        &act(
            "nats_raw_sub",
            json!({ "mode": "sub", "subject": subject.clone() }),
        ),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    client.publish(subject, "not json".into()).await.unwrap();
    client.flush().await.unwrap();

    let outputs = ended.recv().await;
    assert_eq!(outputs.len(), 1, "one run reports: {outputs:?}");
    assert!(!outputs[0].0, "the subscription completes: {outputs:?}");
    assert_eq!(data_of(&outputs), Some(json!("not json")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_times_out_when_no_message_arrives() {
    let Some(_client) = connect_or_skip().await else {
        return;
    };
    let url = nats_url();
    let (engine, ended) = engine(&url, 100, 1).await;
    let principal = Principal::unrestricted();

    start(
        &engine,
        &principal,
        &act(
            "nats_timeout",
            json!({ "mode": "sub", "subject": subject("silent") }),
        ),
    )
    .await;

    let outputs = ended.recv().await;
    assert!(outputs[0].0, "the wait fails: {outputs:?}");
    assert!(
        outputs[0]
            .1
            .to_string()
            .contains("nats receive timed out after 100 ms"),
        "{:?}",
        outputs[0].1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_without_a_message_is_refused() {
    let Some(_client) = connect_or_skip().await else {
        return;
    };
    let url = nats_url();
    let (engine, ended) = engine(&url, 5_000, 1).await;
    let principal = Principal::unrestricted();

    start(
        &engine,
        &principal,
        &act(
            "nats_no_message",
            json!({ "mode": "pub", "subject": subject("empty") }),
        ),
    )
    .await;

    let outputs = ended.recv().await;
    assert!(outputs[0].0, "{outputs:?}");
    assert!(
        outputs[0].1.to_string().contains("validation error"),
        "{:?}",
        outputs[0].1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_mode_is_refused() {
    let Some(_client) = connect_or_skip().await else {
        return;
    };
    let url = nats_url();
    let (engine, ended) = engine(&url, 5_000, 1).await;
    let principal = Principal::unrestricted();

    start(
        &engine,
        &principal,
        &act(
            "nats_bad_params",
            json!({ "mode": "never", "subject": "x" }),
        ),
    )
    .await;

    let outputs = ended.recv().await;
    assert!(outputs[0].0, "{outputs:?}");
    assert!(
        outputs[0].1.to_string().contains("validation error"),
        "{:?}",
        outputs[0].1
    );
}

/// An unreachable broker fails the act instead of waiting forever: the client
/// is created once and its failure is reported by the act that needed it.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_broker_fails_the_act() {
    // 192.0.2.0/24 is TEST-NET-1: no broker answers there.
    let (engine, ended) = engine("nats://192.0.2.1:4222", 150, 1).await;
    let principal = Principal::unrestricted();

    start(
        &engine,
        &principal,
        &act(
            "nats_unreachable",
            json!({ "mode": "pub", "subject": "x", "message": "1" }),
        ),
    )
    .await;

    let outputs = ended.recv().await;
    assert!(outputs[0].0, "{outputs:?}");
    let text = outputs[0].1.to_string();
    assert!(
        text.contains("failed to connect to NATS") || text.contains("timed out"),
        "{text}"
    );
}

/// A subject the protocol cannot carry fails the publish instead of being
/// sent: the failure is reported by the act that tried.
#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_subject_fails_the_publish() {
    let Some(_client) = connect_or_skip().await else {
        return;
    };
    let (engine, ended) = engine(&nats_url(), 5_000, 1).await;
    let principal = Principal::unrestricted();

    start(
        &engine,
        &principal,
        &act(
            "nats_bad_subject",
            json!({ "mode": "pub", "subject": "bad subject", "message": "hi" }),
        ),
    )
    .await;

    let outputs = ended.recv().await;
    assert!(outputs[0].0, "{outputs:?}");
    assert!(
        outputs[0]
            .1
            .to_string()
            .contains("failed to publish message"),
        "{:?}",
        outputs[0].1
    );
}
