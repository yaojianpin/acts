//! End-to-end proof of what a script's filesystem is.
//!
//! The run's own directory is the interpreter's root: a relative path resolves
//! inside it, a file the host put there is readable, and what the script writes
//! is read back from the host at `<workdir root>/<pid>`. Without a workdir
//! there is no host directory to root it at, and the script runs on the
//! interpreter's own in-memory filesystem instead.

mod support;

use acts::{Engine, Principal};
use acts_package_shell::ShellPackage;
use support::{deploy, engine, run_shell, scratch, start, wait_for, workdir};

/// The root is the run's directory: `pwd` is `/`, the file the host puts there
/// is readable at the same relative path, and a write lands in the run's
/// directory on the host — the two are one file, not a copy.
///
/// The script waits for the seeded file and then holds the run open (`sleep`)
/// while the host reads what it wrote: a finished run's rows, and the directory
/// with them, are swept away.
#[tokio::test]
async fn the_run_directory_is_the_scripts_root() {
    let dir = scratch("root");
    let (engine, principal) = engine(&dir, "").await;

    // The run's directory does not exist until the run starts — it is created
    // for the pid the engine hands out — so the script waits for the file the
    // host is about to put in it. That wait is the read half of the proof: the
    // file appears in the middle of the run and the script sees it.
    let script = "while [ ! -f seed.txt ]; do sleep 0.1; done\ncat seed.txt > seed.out\npwd > pwd.txt\necho written > written.txt\nsleep 30";
    deploy(&engine, &principal, "shell-root", script).await;
    let pid = start(&engine, &principal, "shell-root").await;

    let run_dir = workdir(&dir, &pid);
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("seed.txt"), b"seeded\n").unwrap();

    let held = wait_for(30, || {
        run_dir.join("seed.out").exists()
            && run_dir.join("pwd.txt").exists()
            && run_dir.join("written.txt").exists()
    })
    .await;
    assert!(
        held,
        "the script's writes never reached the host directory {}",
        run_dir.display()
    );

    assert_eq!(
        std::fs::read_to_string(run_dir.join("seed.out")).unwrap(),
        "seeded\n",
        "the host file the run's directory holds must be readable by the script"
    );
    assert_eq!(
        std::fs::read_to_string(run_dir.join("pwd.txt")).unwrap(),
        "/\n",
        "the script's working directory must be the run's directory, at the root"
    );
    assert_eq!(
        std::fs::read_to_string(run_dir.join("written.txt")).unwrap(),
        "written\n",
        "what the script wrote must be the file the host sees"
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// Without a workdir there is no host directory to root the interpreter at: the
/// script still runs, on the interpreter's own in-memory filesystem, which has
/// no host behind it — the write succeeds and leaves nothing on the host.
#[tokio::test]
async fn without_a_workdir_the_script_gets_a_sandbox_with_no_host_behind_it() {
    let dir = scratch("sandbox");
    let engine = Engine::builder()
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap();

    let script = "echo hi > f.txt\ncat f.txt";
    let outcome = run_shell(&engine, &Principal::unrestricted(), "shell-sandbox", script).await;

    assert!(!outcome.failed, "the act itself must still run");
    assert!(
        outcome.outputs.contains("hi"),
        "the script's own filesystem must still work, got: {}",
        outcome.outputs
    );
    assert!(
        !dir.join("shell-sandbox").exists(),
        "a run with no workdir must leave nothing on the host"
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
