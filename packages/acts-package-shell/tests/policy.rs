//! End-to-end proof that the `[shell]` allow/deny lists are enforced on a
//! real run: a script the policy refuses never reaches the shell, so its
//! side effect never happens — while the same script under a policy that
//! admits it does run.

mod support;

use acts::Engine;
use acts_package_shell::ShellPackage;
use support::{config, run_shell, scratch, writer};

/// A denied script is refused before the shell is spawned: the file it would
/// have written does not exist.
#[tokio::test]
async fn a_denied_script_never_runs() {
    let dir = scratch("deny");
    let target = dir.join("pwned.txt");
    let (shell, script) = writer(&target);

    let engine = Engine::builder()
        .set_config(&config("[shell]\ndeny = [\"*pwned*\"]\n"))
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap();

    let failed = run_shell(&engine, "shell-denied", shell, &script)
        .await
        .failed;

    assert!(failed, "the run must fail");
    assert!(
        !target.exists(),
        "the script reached the shell: {} exists",
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
    let target = dir.join("ok.txt");
    let (shell, script) = writer(&target);

    let engine = Engine::builder()
        .set_config(&config("[shell]\nallow = [\"echo *\"]\n"))
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap();

    let failed = run_shell(&engine, "shell-allowed", shell, &script)
        .await
        .failed;

    assert!(!failed, "the run must complete");
    assert!(
        target.exists(),
        "the admitted script did not run: {} is missing",
        target.display()
    );
    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// deny wins over allow, on a run: the same script, listed in both, fails and
/// leaves nothing behind.
#[tokio::test]
async fn deny_wins_on_a_run() {
    let dir = scratch("both");
    let target = dir.join("both.txt");
    let (shell, script) = writer(&target);

    let engine = Engine::builder()
        .set_config(&config(
            "[shell]\nallow = [\"echo *\"]\ndeny = [\"*both.txt*\"]\n",
        ))
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap();

    let failed = run_shell(&engine, "shell-both", shell, &script)
        .await
        .failed;

    assert!(failed, "deny must win over allow");
    assert!(!target.exists(), "{} must not exist", target.display());
    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
