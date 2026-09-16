//! Shared harness for this package's end-to-end tests: deploy one shell act
//! into a real engine, run it, and report how the run ended.
//!
//! Every test here proves its point on a real run rather than on a unit of the
//! package: the act fails or completes, and the side effects a bounded act must
//! not leave behind are checked on the filesystem.
#![allow(dead_code)]

use acts::{Config, Engine, Principal, Signal, Vars, Workflow};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub fn config(toml_text: &str) -> Config {
    Config {
        data: Default::default(),
        table: toml::from_str::<toml::Table>(toml_text).unwrap(),
    }
}

/// A scratch directory under the system temp dir, unique per run.
pub fn scratch(tag: &str) -> PathBuf {
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
pub fn writer(target: &Path) -> (&'static str, String) {
    if cfg!(windows) {
        ("powershell", format!("echo ok > '{}'", target.display()))
    } else {
        ("sh", format!("echo ok > '{}'", target.display()))
    }
}

/// The shell and a script that writes `target` only after `secs` — a marker
/// that must not appear when the act's deadline cut the script short.
pub fn late_writer(target: &Path, secs: u32) -> (&'static str, String) {
    if cfg!(windows) {
        (
            "powershell",
            format!(
                "Start-Sleep -Seconds {secs}; echo late > '{}'",
                target.display()
            ),
        )
    } else {
        (
            "sh",
            format!("sleep {secs}; echo late > '{}'", target.display()),
        )
    }
}

/// The shell and a script that never exits on its own.
pub fn sleeper(secs: u32) -> (&'static str, String) {
    if cfg!(windows) {
        ("powershell", format!("Start-Sleep -Seconds {secs}"))
    } else {
        ("sh", format!("sleep {secs}"))
    }
}

/// The shell and a script that writes more than any capture limit tests use,
/// to `stderr` when `to_stderr` (a stream nobody reads blocks on write, so
/// both streams have to be bounded by themselves).
pub fn flood(to_stderr: bool) -> (&'static str, String) {
    if cfg!(windows) {
        let line = if to_stderr {
            "'xxxxxxxxxxxxxxxxxxxx'; [Console]::Error.WriteLine('xxxxxxxxxxxxxxxxxxxx')"
        } else {
            "'xxxxxxxxxxxxxxxxxxxx'"
        };
        (
            "powershell",
            format!("for ($i = 0; $i -lt 500000; $i++) {{ {line} }}"),
        )
    } else if to_stderr {
        ("sh", "yes xxxxxxxxxxxxxxxxxxxx >&2".to_string())
    } else {
        ("sh", "yes xxxxxxxxxxxxxxxxxxxx".to_string())
    }
}

/// The shell and a script that writes to `stdout` forever.
///
/// Its sibling [`flood`] is a bounded loop on Windows, so it ends by itself
/// and an act that returns proves nothing about what the act did with it; this
/// one can only end by being stopped.
pub fn endless_flood() -> (&'static str, String) {
    if cfg!(windows) {
        (
            "powershell",
            "while ($true) { 'xxxxxxxxxxxxxxxxxxxx' }".to_string(),
        )
    } else {
        ("sh", "yes xxxxxxxxxxxxxxxxxxxx".to_string())
    }
}

/// The same endless writer on `stderr` alone — the stream nobody drains, which
/// the writing script blocks on unless the act reads it too.
pub fn endless_stderr_flood() -> (&'static str, String) {
    if cfg!(windows) {
        (
            "powershell",
            "while ($true) { [Console]::Error.WriteLine('xxxxxxxxxxxxxxxxxxxx') }".to_string(),
        )
    } else {
        ("sh", "yes xxxxxxxxxxxxxxxxxxxx >&2".to_string())
    }
}

/// The shell and a script that ends at once with `ok` on `stdout`: the
/// ordinary act that has to still run after a flood.
pub fn quick() -> (&'static str, String) {
    if cfg!(windows) {
        ("powershell", "'ok'".to_string())
    } else {
        ("sh", "echo ok".to_string())
    }
}

/// How a shell run ended: whether it failed, how long it took, and the pid
/// the engine gave it — the act's own failure lands in that process's tasks.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub failed: bool,
    pub elapsed: Duration,
    pub pid: String,
}

/// Run one shell act to completion, or to the failure that ends it early.
pub async fn run_shell(engine: &Engine, mid: &str, shell: &str, script: &str) -> Outcome {
    run_shells(engine, &[mid], shell, script)
        .await
        .pop()
        .expect("one run reports one outcome")
}

/// Run `mids.len()` copies of one shell act at once and report how each ended.
///
/// Every handler on the channel sees every event, so a per-run signal cannot
/// tell whose end it was: the pid in the event is what identifies the run, and
/// the wait is counted — the signal fires once the `count`-th terminal event
/// has been recorded, and the outcomes are read back by pid.
pub async fn run_shells(engine: &Engine, mids: &[&str], shell: &str, script: &str) -> Vec<Outcome> {
    let executor = engine.executor(&Principal::unrestricted());
    for mid in mids {
        // The step is built from text rather than with a closure: `with_step`
        // takes a plain `fn`, and the script is a parameter of this test.
        let workflow = Workflow::from_yml(&format!(
            "name: shell run\nid: {mid}\nver: \"0.1.0\"\nsteps:\n  - id: s1\n    uses: acts.app.shell\n    params:\n      shell: {shell}\n      script: |\n        {script}\n"
        ))
        .unwrap();
        executor.model().deploy(&workflow, None).await.unwrap();
    }

    // Both handlers are installed before any start, so no outcome is missed.
    let count = mids.len();
    let done = Arc::new(AtomicUsize::new(0));
    let ended = engine.signal::<Vec<Outcome>>(Vec::new());
    let (on_error, on_complete) = ended.double();
    let (error_done, complete_done) = (done.clone(), done.clone());
    let start = Instant::now();
    engine.channel().on_error(move |e| {
        let (ended, done, pid) = (on_error.clone(), error_done.clone(), e.pid.clone());
        async move { record(&ended, &done, count, true, start, pid) }
    });
    engine.channel().on_complete(move |e| {
        let (ended, done, pid) = (on_complete.clone(), complete_done.clone(), e.pid.clone());
        async move { record(&ended, &done, count, false, start, pid) }
    });

    for mid in mids {
        executor.proc().start(mid, Vars::new()).await.unwrap();
    }

    ended.recv().await
}

/// Record one finished run, and fire the signal once every run has reported.
fn record(
    ended: &Signal<Vec<Outcome>>,
    done: &AtomicUsize,
    count: usize,
    failed: bool,
    start: Instant,
    pid: String,
) {
    ended.update(|outcomes| {
        outcomes.push(Outcome {
            failed,
            elapsed: start.elapsed(),
            // `update` takes an `Fn`, so a captured pid cannot be moved out of
            // the closure; it is cloned into the outcome instead.
            pid: pid.clone(),
        })
    });
    // The push precedes this count, so the `count`-th one closes a signal that
    // already holds every outcome.
    if done.fetch_add(1, Ordering::SeqCst) + 1 == count {
        ended.close();
    }
}

/// The vars of every task a finished run persisted, as text — the failure a
/// package reported (`ecode`/`message`) lands in the act task's own vars.
pub async fn run_vars(engine: &Engine, pid: &str) -> String {
    let info = engine
        .executor(&Principal::unrestricted())
        .proc()
        .get(pid)
        .await
        .unwrap();
    info.tasks
        .iter()
        .map(|task| task.data.clone())
        .collect::<Vec<_>>()
        .join("\n")
}
