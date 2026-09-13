//! Startup behavior of the `acts-server` binary: an unusable log directory is
//! a configuration/deployment error, so the process must exit with a
//! diagnostic error instead of panicking — a supervisor has to be able to see
//! *why* the server did not start.

use std::{
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "acts-server-startup-{}-{}-{}",
        std::process::id(),
        name,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn unusable_log_dir_fails_without_panicking() {
    let dir = scratch("bad-log-dir");
    // A regular file can never be a log directory: `create_dir_all` rejects it
    // with "already exists" / ENOTDIR instead of the server dying on a panic.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"").unwrap();

    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    // TOML literal string: the windows path separators stay verbatim.
    std::fs::write(
        config_dir.join("acts.toml"),
        format!("[log]\ndir = '{}'\nlevel = \"INFO\"\n", blocker.display()),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_acts-server"))
        .current_dir(&dir)
        .env("ACTS_CONFIG_DIR", &config_dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "server exited successfully with an unusable log dir: {stderr}"
    );
    assert!(
        stderr.contains("failed to create log dir"),
        "no diagnostic for the unusable log dir in stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "startup panicked instead of reporting an error: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
