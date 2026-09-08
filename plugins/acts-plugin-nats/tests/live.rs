//! Live NATS tests for `acts-plugin-nats`.
//!
//! Requires a reachable NATS server: `ACTS_NATS_URL` (default
//! `nats://127.0.0.1:4222`). When no server responds the tests skip — keep
//! plain `cargo test` green without NATS. CI runs them against a NATS
//! service container.

use acts::{Config, Engine, Vars, Workflow};
use acts_plugin_nats::NatsPlugin;
use async_nats::Client;
use futures_util::StreamExt;
use serde_json::{Value as JsonValue, json};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
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
fn temp_config(nats_section: &str) -> (PathBuf, Config) {
    let path = std::env::temp_dir().join(format!(
        "acts_nats_test_{}_{}.toml",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        format!("[log]\ndir = \"acts-nats-test-log\"\nlevel = \"INFO\"\n\n{nats_section}"),
    )
    .unwrap();
    let config = Config::create(&path);
    (path, config)
}

fn engine_with_nats(config: &Config) -> Engine {
    Engine::builder()
        .set_config(config)
        .add_plugin(&NatsPlugin::new())
        .build()
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

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_actions_over_nats() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let (path, config) =
        temp_config("[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts\"\n");
    let engine = engine_with_nats(&config).start().await.unwrap();

    // upsert
    let reply = request_action(
        &client,
        "acts.cmd".to_string(),
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
        "acts.cmd".to_string(),
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
        "acts.cmd".to_string(),
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
        "[nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts\"\n\n\
         [[nats.channels]]\n\
         id = \"t2\"\n\
         subject = \"acts.evt.t2\"\n\
         type = \"*\"\n\
         state = \"*\"\n\
         uses = \"*\"\n",
    );
    let engine = engine_with_nats(&config).start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut sub = client.subscribe("acts.evt.t2").await.unwrap();
    let executor = engine.executor();
    let workflow = Workflow::new()
        .with_id("nats_event_demo")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses_code("acts.transform.code", r#"$set("output", "done");"#)
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
