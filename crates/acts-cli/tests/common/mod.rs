//! The shared test harness: boot the server `acts-server` assembles — the
//! shipped engine with the shipped gRPC plugin — for one case.
//!
//! Every case boots its own server on a port picked for it, so two cases run
//! in parallel; the engine and the scratch directory it ran in are torn down
//! when the body returns.

use acts::{Config, Engine};
use acts_acl::AclUsers;
use acts_plugin_grpc::GrpcPlugin;
use anyhow::Context;
use std::{
    future::Future,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Boot a server for one case and hand its engine and endpoint to `body`.
///
/// `disable_acl` selects the ACL the engine runs under: a case that only
/// exercises the catalogue runs with it disabled — every caller is
/// unrestricted, which is what an embedder gets from
/// `EngineBuilder::disable_acl` — while a case that exercises the user model
/// installs the store-backed registry (`with_user_acl`), declares users on
/// `engine.acl()` and logs in over the channel.
pub async fn with_server<F, Fut>(name: &str, disable_acl: bool, body: F) -> anyhow::Result<()>
where
    F: FnOnce(Engine, String) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    // The port is picked before the server can bind it, so a collision (the
    // same ephemeral port handed to a sibling case right after it released it)
    // is retried on a fresh port instead of failing on a listener that never
    // came up. A case that fails for its own reason still fails once — the
    // retry is only for the port.
    let mut last = None;
    for _ in 0..3 {
        let dir = scratch(name)?;
        let port = free_port()?;
        let config_path = dir.join("acts.toml");
        // The ACL is selected on the builder, not in the config: an `[acl]`
        // section is no longer read (users live in the store), so the cases
        // that need no credentials say so by disabling the ACL.
        std::fs::write(&config_path, format!("[grpc]\nport = {port}\n"))
            .with_context(|| format!("failed to write {}", config_path.display()))?;

        // the engine and the transport plugin are the shipped ones — a CLI test
        // must not be checking a second, hand-rolled wiring that can drift
        let config = Config::create(&config_path)
            .with_context(|| format!("failed to load {}", config_path.display()))?;
        let mut builder = Engine::builder()
            .set_config(&config)
            .add_plugin(&GrpcPlugin::new());
        // The user registry is a crate of its own, installed on the builder: a
        // case that needs users gets a registry it can declare them in, and a
        // case that needs no restrictions opts the ACL out entirely.
        if disable_acl {
            builder = builder.disable_acl();
        } else {
            builder = builder.with_user_acl();
        }
        let engine = builder
            .start()
            .await
            .context("the test server must start")?;

        let url = format!("http://127.0.0.1:{port}");
        if let Err(err) = wait_for_server(&url).await {
            engine.close().await;
            let _ = std::fs::remove_dir_all(&dir);
            last = Some(err);
            continue;
        }

        let result = body(engine.clone(), url).await;
        engine.close().await;
        let _ = std::fs::remove_dir_all(&dir);
        return result;
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("the server never accepted a connection")))
}

/// How long a case waits for the server to accept a connection. The gRPC
/// listener binds in a spawned task, and CI runs this suite instrumented
/// (`cargo llvm-cov`) on a shared runner, so the budget is generous: waiting
/// it out means something is actually wrong.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

async fn wait_for_server(url: &str) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        match acts_cli::client::connect(url, None).await {
            Ok(_) => return Ok(()),
            Err(err) => last = err.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err(anyhow::anyhow!(
        "{url} never accepted a connection within {READY_TIMEOUT:?}: {last}"
    ))
}

fn scratch(name: &str) -> anyhow::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("the clock is before the epoch: {err}"))?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("acts-cli-{}-{name}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

fn free_port() -> anyhow::Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("failed to bind a free port")?;
    let port = listener
        .local_addr()
        .context("failed to read the bound address")?
        .port();
    drop(listener);
    Ok(port)
}
