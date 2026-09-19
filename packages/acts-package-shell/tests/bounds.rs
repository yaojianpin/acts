//! End-to-end proof that a shell act is bounded on a real run: a script that
//! waits past the `[shell]` deadline fails at it, a script that floods a stream
//! fails at the capture limit, and neither takes a scheduler lane with it.

mod support;

use std::time::Duration;
use support::{
    engine, engine_with_lanes, flood, late_writer, quick, run_shell, run_shells, scratch, workdir,
};

/// A script that waits past the deployment's deadline fails instead of holding
/// the run (and a scheduler lane) for as long as it likes — and the work it
/// would have done after waking never happens: the file it writes when it gets
/// there never appears.
#[tokio::test]
async fn a_script_that_never_exits_fails_at_the_deadline() {
    let dir = scratch("deadline");
    let (engine, principal) = engine(&dir, "[shell]\ntimeout-ms = 300\n").await;

    let script = late_writer("late.txt", 2);
    let outcome = run_shell(&engine, &principal, "shell-deadline", &script).await;

    assert!(outcome.failed, "the run must fail");
    let elapsed = outcome.elapsed;
    assert!(
        elapsed < Duration::from_secs(10),
        "the act must not wait for the script: took {elapsed:?}"
    );
    assert!(
        outcome.outputs.contains("timed out after 300 ms"),
        "the failure must name the deadline, got: {}",
        outcome.outputs
    );

    // Outlive the script's own sleep: a script that was only abandoned (not
    // stopped) would reach its write and leave the file behind.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let late = workdir(&dir, &outcome.pid).join("late.txt");
    assert!(
        !late.exists(),
        "the stopped script wrote {} after the act failed",
        late.display()
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A stream that floods is stopped at the deployment's capture limit — on
/// `stdout` and on `stderr`, since each stream is captured and each is bounded
/// by itself.
#[tokio::test]
async fn a_flooding_stdout_is_stopped_at_the_capture_limit() {
    let dir = scratch("flood-out");
    let (engine, principal) = engine(&dir, "[shell]\nmax-output-bytes = 4096\n").await;

    let outcome = run_shell(
        &engine,
        &principal,
        "shell-flood-out",
        &flood(1_000, false),
    )
    .await;

    assert!(outcome.failed, "the run must fail");
    assert!(
        outcome.outputs.contains("max-output-bytes limit (4096)"),
        "the failure must name the capture limit, got: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// The other stream, on its own: `stderr` is captured and capped whether or not
/// the script writes anything to `stdout`.
#[tokio::test]
async fn a_flooding_stderr_is_stopped_at_the_capture_limit() {
    let dir = scratch("flood-err");
    let (engine, principal) = engine(&dir, "[shell]\nmax-output-bytes = 4096\n").await;

    let outcome = run_shell(
        &engine,
        &principal,
        "shell-flood-err",
        &flood(1_000, true),
    )
    .await;

    assert!(outcome.failed, "the run must fail");
    assert!(
        outcome.outputs.contains("max-output-bytes limit (4096)"),
        "the failure must name the capture limit, got: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A `[shell]` section that says nothing about bounds still bounds the act: the
/// package's own defaults are in force, never "no bound".
#[tokio::test]
async fn the_default_capture_limit_bounds_a_stream() {
    let dir = scratch("default-flood");
    let (engine, principal) = engine(&dir, "").await;

    let outcome = run_shell(
        &engine,
        &principal,
        "shell-default-flood",
        &flood(30_000, false),
    )
    .await;

    assert!(outcome.failed, "the run must fail");
    assert!(
        outcome.outputs.contains("max-output-bytes limit (1048576)"),
        "the default limit must be the one the act reports, got: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// The default deadline bounds an act that no section configured, and a script
/// that would finish inside it still succeeds — the bound is not a blanket
/// refusal of shell acts.
#[tokio::test]
async fn the_default_deadline_does_not_refuse_a_quick_script() {
    let dir = scratch("default-quick");
    let (engine, principal) = engine(&dir, "").await;

    let outcome = run_shell(&engine, &principal, "shell-default-quick", &quick()).await;

    assert!(
        !outcome.failed,
        "a script inside the deadline must complete: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains("ok"),
        "the script's output must reach the act, got: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A script the deadline stopped gives its lane back: the engine has a single
/// lane here, so the ordinary act that runs afterwards can only have run on the
/// lane the stopped script held.
#[tokio::test]
async fn a_stopped_script_releases_its_lane() {
    let dir = scratch("lane");
    let (engine, principal) = engine_with_lanes(&dir, "[shell]\ntimeout-ms = 300\n", 1).await;

    let script = late_writer("late.txt", 2);
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, &principal, "shell-lane-deadline", &script),
    )
    .await
    .expect("the act must fail at the deadline, not wait for the script");

    assert!(outcome.failed, "the run must fail");
    assert!(
        outcome.outputs.contains("timed out after 300 ms"),
        "the failure must name the deadline, got: {}",
        outcome.outputs
    );

    let after = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, &principal, "shell-after-deadline", &quick()),
    )
    .await
    .expect("the lane the stopped script held must be released");
    assert!(
        !after.failed,
        "an act after the failure must complete: {}",
        after.outputs
    );
    assert!(
        after.outputs.contains("ok"),
        "the act after the failure must have run and reported its output, got: {}",
        after.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// Every lane can be flooding at once and the worker still comes back: four
/// floods on four lanes all fail at the capture limit — the bound is one act's,
/// not the engine's — and the same engine runs an ordinary act afterwards, on
/// the lanes they held.
#[tokio::test]
async fn floods_on_every_lane_leave_the_worker_running() {
    const LANES: usize = 4;
    let dir = scratch("lanes");
    let (engine, principal) =
        engine_with_lanes(&dir, "[shell]\nmax-output-bytes = 4096\n", LANES).await;
    let script = flood(1_000, false);

    let outcomes = tokio::time::timeout(
        Duration::from_secs(60),
        run_shells(
            &engine,
            &principal,
            &[
                "shell-flood-lane-0",
                "shell-flood-lane-1",
                "shell-flood-lane-2",
                "shell-flood-lane-3",
            ],
            &script,
        ),
    )
    .await
    .expect("floods on every lane must fail at the limit, not wait for their scripts");

    assert_eq!(outcomes.len(), LANES, "every flood must report an outcome");
    for outcome in &outcomes {
        assert!(outcome.failed, "every flood must fail");
        assert!(
            outcome.outputs.contains("max-output-bytes limit (4096)"),
            "each failure must name the capture limit, got: {}",
            outcome.outputs
        );
    }

    let after = tokio::time::timeout(
        Duration::from_secs(30),
        run_shell(&engine, &principal, "shell-after-lane-floods", &quick()),
    )
    .await
    .expect("the lanes the floods held must be released");
    assert!(
        !after.failed,
        "an act after the floods must complete: {}",
        after.outputs
    );
    assert!(
        after.outputs.contains("ok"),
        "the act after the floods must have run and reported its output, got: {}",
        after.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
