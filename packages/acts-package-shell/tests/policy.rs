//! End-to-end proof that the `[shell]` allow/deny lists are enforced on a
//! real run: a script the policy refuses never reaches the interpreter, so its
//! side effect never happens — while the same script under a policy that
//! admits it does run.

mod support;

use support::{engine, run_shell, scratch, workdir, writer};

/// The policy's own words, as the act reports them.
const REFUSED: &str = "refused by the [shell] policy";

/// A denied script is refused before anything runs: the file it would have
/// written does not exist, and the act says why.
#[tokio::test]
async fn a_denied_script_never_runs() {
    let dir = scratch("deny");
    let (engine, principal) = engine(&dir, "[shell]\ndeny = [\"*pwned*\"]\n").await;

    let outcome = run_shell(&engine, &principal, "shell-denied", &writer("pwned.txt")).await;

    assert!(outcome.failed, "the run must fail");
    assert!(
        outcome.outputs.contains(REFUSED),
        "the failure must name the policy, got: {}",
        outcome.outputs
    );
    let target = workdir(&dir, &outcome.pid).join("pwned.txt");
    assert!(
        !target.exists(),
        "the script ran: {} exists",
        target.display()
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// The same script under a policy that admits it does run — the guard refuses
/// what the lists refuse, not every script.
#[tokio::test]
async fn an_admitted_script_runs() {
    let dir = scratch("allow");
    let (engine, principal) = engine(&dir, "[shell]\nallow = [\"echo *\"]\n").await;

    let outcome = run_shell(&engine, &principal, "shell-allowed", &writer("ok.txt")).await;

    assert!(
        !outcome.failed,
        "the run must complete, got: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains("ok"),
        "the admitted script did not run, got: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// deny wins over allow, on a run: the same script, listed in both, fails and
/// leaves nothing behind.
#[tokio::test]
async fn deny_wins_on_a_run() {
    let dir = scratch("both");
    let (engine, principal) = engine(
        &dir,
        "[shell]\nallow = [\"echo *\"]\ndeny = [\"*both.txt*\"]\n",
    )
    .await;

    let outcome = run_shell(&engine, &principal, "shell-both", &writer("both.txt")).await;

    assert!(outcome.failed, "deny must win over allow");
    assert!(
        outcome.outputs.contains(REFUSED),
        "the failure must name the policy, got: {}",
        outcome.outputs
    );
    let target = workdir(&dir, &outcome.pid).join("both.txt");
    assert!(!target.exists(), "{} must not exist", target.display());

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
