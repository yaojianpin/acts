//! Startup behavior of the `acts-server` binary: an unusable log directory or
//! a `[db]` section that cannot be read is a configuration/deployment error,
//! so the process — and the engine builder it runs — must report it and let a
//! supervisor see *why* the server did not start, instead of panicking.
//!
//! The config of every case is loaded the way the binary loads it
//! (`acts_server::Config::create` on a real `acts.toml`), so what is exercised
//! is the server's own config path, not a table assembled by hand here.

use acts_server::{Config, ServerPlugins, engine_builder};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

fn scratch(name: &str) -> anyhow::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("the clock is before the epoch: {err}"))?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "acts-server-startup-{}-{name}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Write the config a case runs on and load it the way the binary does.
/// Returns the config directory (what `ACTS_CONFIG_DIR` points at for the
/// cases that run the binary) and the loaded config.
fn server_config(dir: &Path, body: &str) -> anyhow::Result<(PathBuf, Config)> {
    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let path = config_dir.join("acts.toml");
    std::fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    let config =
        Config::create(&path).with_context(|| format!("failed to load {}", path.display()))?;
    Ok((config_dir, config))
}

/// Run the server binary on `config_dir` and return its stderr.
fn run_server(dir: &Path, config_dir: &Path) -> anyhow::Result<String> {
    let bin = env!("CARGO_BIN_EXE_acts-server");
    let output = Command::new(bin)
        .current_dir(dir)
        .env("ACTS_CONFIG_DIR", config_dir)
        .output()
        .with_context(|| format!("failed to run {bin}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "the server exited successfully on an unusable config: {stderr}"
    );
    Ok(stderr)
}

#[test]
fn unusable_log_dir_fails_without_panicking() -> anyhow::Result<()> {
    let dir = scratch("bad-log-dir")?;
    // A regular file can never be a log directory: `create_dir_all` rejects it
    // with "already exists" / ENOTDIR instead of the server dying on a panic.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"")
        .with_context(|| format!("failed to write {}", blocker.display()))?;

    // TOML literal string: the windows path separators stay verbatim.
    let (config_dir, _) = server_config(
        &dir,
        &format!("[log]\ndir = '{}'\nlevel = \"INFO\"\n", blocker.display()),
    )?;

    let stderr = run_server(&dir, &config_dir)?;
    assert!(
        stderr.contains("failed to create log dir"),
        "no diagnostic for the unusable log dir in stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "startup panicked instead of reporting an error: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// A `[db]` section the server cannot deserialize is refused with a
/// diagnostic naming the offending section — before a store is ever opened.
#[test]
fn a_malformed_db_section_is_reported_not_panicked() -> anyhow::Result<()> {
    let dir = scratch("bad-db")?;
    let (config_dir, _) = server_config(&dir, "[db]\ntype = \"nope\"\n")?;

    let stderr = run_server(&dir, &config_dir)?;
    assert!(
        stderr.contains("failed to get 'db' config"),
        "no diagnostic for the malformed [db] in stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "startup panicked instead of reporting an error: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Access control is always on — there is no `[acl]` section to enable it —
/// and the anonymous caller is refused everything outside the catalogue.
#[tokio::test(flavor = "multi_thread")]
async fn the_acl_is_on_by_default_and_refuses_anonymous() -> anyhow::Result<()> {
    let dir = scratch("acl-default")?;
    let (_, config) = server_config(&dir, "")?;

    let engine = engine_builder(
        &config,
        Arc::new(acts::MemoryStore::new()),
        &ServerPlugins::default(),
    )?
    .start()
    .await?;

    assert!(engine.acl().enabled());
    let err = acts::actions::apply(&engine, "model:rm", acts::Vars::new().with("id", "x"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, acts::actions::Error::Unauthenticated(_)),
        "got: {err}"
    );

    // the catalogue is the anonymous grant: which models exist stays readable
    acts::actions::apply(&engine, "model:ls", acts::Vars::new())
        .await
        .context("the catalogue must stay readable to the anonymous caller")?;

    engine.close().await;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
