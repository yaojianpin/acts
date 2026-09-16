//! End-to-end proof that a shell act is bounded on a real run: a script that
//! never exits is killed at the `[shell]` deadline, a script that floods a
//! stream is killed at the capture limit, and neither leaves work behind.

mod support;

use acts::Engine;
use acts_package_shell::ShellPackage;
use std::time::Duration;
use support::{config, flood, late_writer, run_shell, run_vars, scratch, sleeper};

/// The engine whose `[shell]` section is `toml_text`.
async fn engine(toml_text: &str) -> Engine {
    Engine::builder()
        .set_config(&config(toml_text))
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap()
}

/// A script that never stops fails at the deployment's deadline instead of
/// holding the run (and a scheduler lane) for as long as it likes — and the
/// shell it started is gone: the file it would have written when it woke up
/// never appears.
#[tokio::test]
async fn a_script_that_never_exits_fails_at_the_deadline() {
    let dir = scratch("deadline");
    let target = dir.join("late.txt");
    let (shell, script) = late_writer(&target, 2);
    let engine = engine("[shell]\ntimeout-ms = 300\n").await;

    let outcome = run_shell(&engine, "shell-deadline", shell, &script).await;

    assert!(outcome.failed, "the run must fail");
    let elapsed = outcome.elapsed;
    assert!(
        elapsed < Duration::from_secs(10),
        "the act must not wait for the script: took {elapsed:?}"
    );
    assert!(
        run_vars(&engine, &outcome.pid)
            .await
            .contains("timed out after 300 ms"),
        "the failure must name the deadline"
    );

    // outlive the script's own sleep: a shell that was only abandoned (not
    // killed) writes the file and proves it was still running
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(
        !target.exists(),
        "the killed script wrote {} after the act failed",
        target.display()
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A stream that floods is stopped at the deployment's capture limit — on
/// stderr as well as on stdout, since a stream nobody drains blocks the
/// writing script.
#[tokio::test]
async fn a_flooding_stream_is_stopped_at_the_capture_limit() {
    let (shell, script) = flood(true);
    let engine = engine("[shell]\nmax-output-bytes = 4096\n").await;

    let outcome = run_shell(&engine, "shell-flood", shell, &script).await;

    assert!(outcome.failed, "the run must fail");
    assert!(
        run_vars(&engine, &outcome.pid)
            .await
            .contains("max-output-bytes limit (4096)"),
        "the failure must name the capture limit"
    );

    engine.close().await;
}

/// A `[shell]` section that says nothing about bounds still bounds the act:
/// the package's own defaults are in force, never "no bound".
#[tokio::test]
async fn the_default_capture_limit_bounds_a_stream() {
    let (shell, script) = flood(false);
    let engine = engine("").await;

    let outcome = run_shell(&engine, "shell-default-flood", shell, &script).await;

    assert!(outcome.failed, "the run must fail");
    assert!(
        run_vars(&engine, &outcome.pid)
            .await
            .contains("max-output-bytes limit (1048576)"),
        "the default limit must be the one the act reports"
    );

    engine.close().await;
}

/// The default deadline bounds an act that no section configured, and a script
/// that would finish inside it still succeeds — the bound is not a blanket
/// refusal of shell acts.
#[tokio::test]
async fn the_default_deadline_does_not_refuse_a_quick_script() {
    let (shell, script) = sleeper(0);
    let engine = engine("").await;

    let outcome = run_shell(&engine, "shell-default-quick", shell, &script).await;

    assert!(
        !outcome.failed,
        "a script inside the deadline must complete"
    );

    engine.close().await;
}
