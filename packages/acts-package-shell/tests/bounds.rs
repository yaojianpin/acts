//! End-to-end proof that a shell act is bounded on a real run: a script that
//! never exits is killed at the `[shell]` deadline, a script that floods a
//! stream is killed at the capture limit, and neither leaves work behind.

mod support;

use acts::Engine;
use acts_package_shell::ShellPackage;
use std::time::Duration;
use support::{
    config, endless_flood, endless_stderr_flood, flood, late_writer, quick, run_shell, run_shells,
    run_vars, scratch, sleeper,
};

/// The engine whose `[shell]` section is `toml_text`.
async fn engine(toml_text: &str) -> Engine {
    Engine::builder()
        .set_config(&config(toml_text))
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap()
}

/// The same engine with its scheduler lanes pinned: a claim about a lane ("the
/// flood gave it back") is only a claim when a flood can occupy every lane
/// there is.
async fn engine_with_lanes(toml_text: &str, lanes: usize) -> Engine {
    Engine::builder()
        .set_config(&config(toml_text))
        .scheduler_workers(lanes)
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

/// A script that never stops writing is stopped at the capture limit and gives
/// its lane back: the engine has a single lane here, so the ordinary act that
/// runs afterwards can only have run on the lane the flood held.
///
/// The script cannot end by itself — the sibling helper `flood` is a bounded
/// loop on Windows, which is why that path had no coverage there — so an act
/// that waited for the writer would never return, and the guard turns that
/// regression into a failure instead of a hung suite.
#[tokio::test]
async fn a_flood_that_never_stops_releases_the_lane() {
    let engine = engine_with_lanes("[shell]\nmax-output-bytes = 4096\n", 1).await;
    let (shell, script) = endless_flood();

    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, "shell-endless-flood", shell, &script),
    )
    .await
    .expect("the act must fail at the capture limit, not wait for a script that never stops");

    assert!(outcome.failed, "the run must fail");
    assert!(
        run_vars(&engine, &outcome.pid)
            .await
            .contains("max-output-bytes limit (4096)"),
        "the failure must name the capture limit"
    );

    let (shell, script) = quick();
    let after = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, "shell-after-flood", shell, &script),
    )
    .await
    .expect("the lane the flood held must be released");
    assert!(!after.failed, "an act after the flood must complete");
    assert!(
        run_vars(&engine, &after.pid).await.contains("ok"),
        "the act after the flood must have run and reported its output"
    );

    engine.close().await;
}

/// The stream nobody drains is bounded by itself: an endless writer on
/// `stderr` fails the act at the capture limit instead of blocking on a full
/// pipe, which is what an act that reads only the stream it cares about does.
///
/// On Windows `flood(true)` writes both streams while Unix's writes `stderr`
/// alone, so this is the missing half: a script flooding `stderr` only, with a
/// writer that never stops on either platform.
#[tokio::test]
async fn an_endless_flood_on_stderr_fails_at_the_limit() {
    let engine = engine_with_lanes("[shell]\nmax-output-bytes = 4096\n", 1).await;
    let (shell, script) = endless_stderr_flood();

    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, "shell-endless-stderr", shell, &script),
    )
    .await
    .expect("a flood on stderr must fail the act, not block on the full pipe");

    assert!(outcome.failed, "the run must fail");
    assert!(
        run_vars(&engine, &outcome.pid)
            .await
            .contains("max-output-bytes limit (4096)"),
        "the failure must name the capture limit"
    );

    engine.close().await;
}

/// Every lane can be flooding at once and the worker still comes back: four
/// floods on four lanes all fail at the capture limit — the bound is one act's,
/// not the engine's — and the same engine runs an ordinary act afterwards, on
/// the lanes they held.
#[tokio::test]
async fn floods_on_every_lane_leave_the_worker_running() {
    const LANES: usize = 4;
    let engine = engine_with_lanes("[shell]\nmax-output-bytes = 4096\n", LANES).await;
    let (shell, script) = endless_flood();

    let outcomes = tokio::time::timeout(
        Duration::from_secs(60),
        run_shells(
            &engine,
            &[
                "shell-flood-lane-0",
                "shell-flood-lane-1",
                "shell-flood-lane-2",
                "shell-flood-lane-3",
            ],
            shell,
            &script,
        ),
    )
    .await
    .expect("floods on every lane must fail at the limit, not wait for their scripts");

    assert_eq!(outcomes.len(), LANES, "every flood must report an outcome");
    for outcome in &outcomes {
        assert!(outcome.failed, "every flood must fail");
        assert!(
            run_vars(&engine, &outcome.pid)
                .await
                .contains("max-output-bytes limit (4096)"),
            "each failure must name the capture limit"
        );
    }

    let (shell, script) = quick();
    let after = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, "shell-after-lane-floods", shell, &script),
    )
    .await
    .expect("the lanes the floods held must be released");
    assert!(!after.failed, "an act after the floods must complete");
    assert!(
        run_vars(&engine, &after.pid).await.contains("ok"),
        "the act after the floods must have run and reported its output"
    );

    engine.close().await;
}
