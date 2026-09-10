//! Live NATS test of the `acts-server` wiring: the server binary registers
//! `acts-plugin-nats` when its config has a `[nats]` section — this boots
//! the same engine builder `acts-server` uses and talks to it over NATS.
//!
//! Requires a reachable NATS server: `ACTS_NATS_URL` (default
//! `nats://127.0.0.1:4222`); skips when none responds so plain `cargo test`
//! stays green. CI runs it against a NATS service container.

use acts::{Config, MemoryStore};
use acts_server::{ServerPlugins, build_engine};
use async_nats::Client;
use serde_json::{Value as JsonValue, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

async fn connect_or_skip() -> Option<Client> {
    let url =
        std::env::var("ACTS_NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
    match tokio::time::timeout(Duration::from_secs(2), async_nats::connect(&url)).await {
        Ok(Ok(client)) => Some(client),
        _ => {
            eprintln!("skip: no NATS server reachable at {url}");
            None
        }
    }
}

async fn request_action(client: &Client, payload: JsonValue) -> JsonValue {
    let payload = serde_json::to_string(&payload).unwrap();
    for _ in 0..20 {
        match tokio::time::timeout(
            Duration::from_secs(2),
            client.request("acts.cmd", payload.clone().into()),
        )
        .await
        {
            Ok(Ok(msg)) => return serde_json::from_slice(&msg.payload).unwrap(),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    panic!("no reply from acts.cmd");
}

#[tokio::test(flavor = "multi_thread")]
async fn acts_server_handles_nats_snapshot_actions() {
    let Some(client) = connect_or_skip().await else {
        return;
    };

    let path: PathBuf = std::env::temp_dir().join(format!(
        "acts_server_nats_{}_{}.toml",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        "[log]\ndir = \"acts-server-test-log\"\nlevel = \"INFO\"\n\n\
         [nats]\nurl = \"nats://127.0.0.1:4222\"\nsubject = \"acts\"\n",
    )
    .unwrap();
    let config = Config::create(&path).unwrap();

    // server plugins: NATS only — the same code path `acts-server` runs
    let engine = build_engine(
        &config,
        Arc::new(MemoryStore::new()),
        &ServerPlugins {
            grpc: false,
            web: false,
            nats: true,
        },
    )
    .start()
    .await
    .unwrap();

    let reply = request_action(
        &client,
        json!({
            "name": "snap:upsert",
            "seq": "req-1",
            "data": {
                "name": "profile",
                "scope": "u1",
                "rev": 9,
                "data": { "val": "y" }
            }
        }),
    )
    .await;
    assert_eq!(reply["data"], json!(true));
    let entry = engine.snapshot().read("profile", "u1").unwrap();
    assert_eq!(entry.rev, 9);
    assert_eq!(entry.data.get::<String>("val").unwrap(), "y");

    let reply = request_action(
        &client,
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
