//! End-to-end proof that the `[shell]` allow/deny lists are enforced on a
//! real run: a script the policy refuses never reaches the shell, so its
//! side effect never happens — while the same script under a policy that
//! admits it does run.

use acts::{Config, Engine, Principal, Vars, Workflow};
use acts_package_shell::ShellPackage;
use std::path::{Path, PathBuf};

fn config(toml_text: &str) -> Config {
    Config {
        data: Default::default(),
        table: toml::from_str::<toml::Table>(toml_text).unwrap(),
    }
}

/// A scratch directory under the system temp dir, unique per run.
fn scratch(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("acts-shell-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The shell and the script that creates `target` — each platform's own, so
/// the path reaches a shell that reads it as a path.
fn writer(target: &Path) -> (&'static str, String) {
    if cfg!(windows) {
        ("powershell", format!("echo ok > '{}'", target.display()))
    } else {
        ("sh", format!("echo ok > '{}'", target.display()))
    }
}

/// Run one shell step to completion; `true` when the run failed.
async fn run_shell(engine: &Engine, mid: &str, shell: &str, script: &str) -> bool {
    // The step is built from text rather than with a closure: `with_step`
    // takes a plain `fn`, and the script is a parameter of this test.
    let workflow = Workflow::from_yml(&format!(
        "name: shell policy\nid: {mid}\nver: \"0.1.0\"\nsteps:\n  - id: s1\n    uses: acts.app.shell\n    params:\n      shell: {shell}\n      script: |\n        {script}\n"
    ))
    .unwrap();

    let executor = engine.executor(&Principal::unrestricted());
    executor.model().deploy(&workflow, None).await.unwrap();

    // Both handlers are installed before the start, so neither outcome can be
    // missed; the first one to fire decides.
    let (fail, failed) = engine.signal::<bool>(false).double();
    let (done, completed) = engine.signal::<bool>(false).double();
    engine.channel().on_error(move |_| {
        let fail = fail.clone();
        async move {
            fail.send(true);
        }
    });
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move {
            done.send(true);
        }
    });

    executor.proc().start(mid, Vars::new()).await.unwrap();
    tokio::select! {
        _ = failed.recv() => true,
        _ = completed.recv() => false,
    }
}

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

    let failed = run_shell(&engine, "shell-denied", shell, &script).await;

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

    let failed = run_shell(&engine, "shell-allowed", shell, &script).await;

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

    let failed = run_shell(&engine, "shell-both", shell, &script).await;

    assert!(failed, "deny must win over allow");
    assert!(!target.exists(), "{} must not exist", target.display());
    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
