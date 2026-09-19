//! Shared harness for this package's end-to-end tests: give a run a workdir,
//! deploy one shell act into a real engine, run it, and report how the run
//! ended.
//!
//! Every test here proves its point on a real run rather than on a unit of the
//! package. What a test reads back is either the run's terminal event — the
//! engine's own report of it, act output and failure message included — or a
//! file in `<workdir root>/<pid>`, the directory the host and the script both
//! see. The run's rows are not evidence: a finished run is swept together with
//! the directory it ran in, so a test that read them would race the sweeper.
#![allow(dead_code)]

use acts::{Config, Engine, KvStore, Principal, Signal, Vars, Workflow};
use acts_package_shell::ShellPackage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The token the harness's role carries. The role is an administrator, so the
/// tests start and deploy like any caller; what they need from it is the ACL's
/// `workdir` root, which is what gives a run the directory its script sees as
/// `/` — a run started without one has no host directory to run in.
const TOKEN: &str = "shell-test";

pub fn config(toml_text: &str) -> Config {
    Config {
        data: Default::default(),
        table: toml::from_str::<toml::Table>(toml_text).unwrap(),
    }
}

/// A scratch directory under the system temp dir, unique per run. It is the
/// ACL's `workdir` root: each run's own directory is `scratch(tag)/<pid>`.
pub fn scratch(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("acts-shell-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The run's own directory as the host sees it — the same directory the script
/// sees as `/`.
pub fn workdir(root: &Path, pid: &str) -> PathBuf {
    root.join(pid)
}

/// An engine over `root` with the shell package deployed, and the principal to
/// start a run through. `toml_text` is the rest of the config: the `[shell]`
/// section a test means to exercise.
pub async fn engine(root: &Path, toml_text: &str) -> (Engine, Principal) {
    engine_with(root, toml_text, None, None).await
}

/// The same engine with its scheduler lanes pinned: a claim about a lane ("the
/// failed act gave it back") is only a claim when the act can occupy every lane
/// there is.
pub async fn engine_with_lanes(root: &Path, toml_text: &str, lanes: usize) -> (Engine, Principal) {
    engine_with(root, toml_text, Some(lanes), None).await
}

/// The same engine over the caller's store backend: the reliability cells own
/// the store the run's rows land in.
pub async fn engine_on(root: &Path, toml_text: &str, store: Arc<dyn KvStore>) -> (Engine, Principal) {
    engine_with(root, toml_text, None, Some(store)).await
}

async fn engine_with(
    root: &Path,
    toml_text: &str,
    lanes: Option<usize>,
    store: Option<Arc<dyn KvStore>>,
) -> (Engine, Principal) {
    let config = config(&format!(
        "[acl]\ntoken = \"{TOKEN}\"\nworkdir = '{}'\n{toml_text}\n",
        root.display()
    ));
    let mut builder = Engine::builder().set_config(&config);
    if let Some(lanes) = lanes {
        builder = builder.scheduler_workers(lanes);
    }
    if let Some(store) = store {
        builder = builder.set_store(store);
    }
    let engine = builder.add_package::<ShellPackage>().start().await.unwrap();
    let principal = engine.acl().authenticate(Some(TOKEN)).unwrap();
    (engine, principal)
}

/// A script that writes `name` in the run's directory, and says `ok` on
/// `stdout`.
pub fn writer(name: &str) -> String {
    format!("echo ok > {name}\necho ok")
}

/// A script that writes `name` only after `secs` — a marker that must not
/// appear when the act's deadline cut the script short.
pub fn late_writer(name: &str, secs: u32) -> String {
    format!("sleep {secs}\necho late > {name}\necho late")
}

/// A script that ends at once with `ok` on `stdout`: the ordinary act that has
/// to still run after a flood.
pub fn quick() -> String {
    "echo ok".to_string()
}

/// A script that writes more than `[shell].max-output-bytes` allows, in two
/// commands.
///
/// `lines` lines of 50 characters is the size the capture limit has to end: a
/// loop would meet the interpreter's own counters first (10,000 commands,
/// 10,000 loop iterations), and this writer is past the default 1 MiB capture
/// at 30,000 lines while staying inside them.
pub fn flood(lines: usize, to_stderr: bool) -> String {
    let pad = "x".repeat(50);
    let to = if to_stderr { " >&2" } else { "" };
    format!("printf '{pad}%s\\n' $(seq 1 {lines}){to}")
}

/// How a shell run ended: whether it failed, how long it took, the pid the
/// engine gave it, and what the terminal event reported.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub failed: bool,
    pub elapsed: Duration,
    pub pid: String,
    /// The terminal event's own vars, as JSON text: the act's `data` on a
    /// completed run, and its `message` — the package's own words — on a
    /// failed one.
    pub outputs: String,
}

/// Deploy the one-step shell act `mid` names: the act step `run_shell` and
/// `run_shells` start, exposed for the tests that drive a run by hand.
pub async fn deploy(engine: &Engine, principal: &Principal, mid: &str, script: &str) {
    // The step is built from text rather than with a closure: `with_step` takes
    // a plain `fn`, and the script is a parameter of this test. The block
    // scalar's lines all have to be indented, whatever the script is.
    let script = script
        .lines()
        .map(|line| format!("        {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let workflow = Workflow::from_yml(&format!(
        "name: shell run\nid: {mid}\nver: \"0.1.0\"\nsteps:\n  - id: s1\n    uses: acts.app.shell\n    params:\n      shell: bash\n      script: |\n{script}\n"
    ))
    .unwrap();
    engine
        .executor(principal)
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();
}

/// Start the deployed run `mid` and answer the pid it runs under — the
/// engine's own pid, so it carries no `-` (the separator an external pid may
/// not contain) and a test that needs the run's directory reads it from here:
/// that directory is `<workdir root>/<pid>`.
///
/// The start is retried a few times because a pid whose writer shard still
/// holds a failed-write ack from an earlier, already-handled fault is refused
/// once — the next flush caller is how the writer reports a store fault, and a
/// caller that treats the fault as transient retries.
pub async fn start(engine: &Engine, principal: &Principal, mid: &str) -> String {
    let executor = engine.executor(principal);
    let mut last = None;
    for _ in 0..5 {
        match executor.proc().start(mid, Vars::new()).await {
            Ok(pid) => return pid,
            Err(err) => {
                last = Some(err);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
    panic!("the run {mid} must start: {}", last.unwrap());
}

/// Run one shell act to completion, or to the failure that ends it early.
pub async fn run_shell(
    engine: &Engine,
    principal: &Principal,
    mid: &str,
    script: &str,
) -> Outcome {
    run_shells(engine, principal, &[mid], script)
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
pub async fn run_shells(
    engine: &Engine,
    principal: &Principal,
    mids: &[&str],
    script: &str,
) -> Vec<Outcome> {
    for mid in mids {
        deploy(engine, principal, mid, script).await;
    }

    // Both handlers are installed before any start, so no outcome is missed.
    let count = mids.len();
    let done = Arc::new(AtomicUsize::new(0));
    let ended = engine.signal::<Vec<Outcome>>(Vec::new());
    let (on_error, on_complete) = ended.double();
    let (error_done, complete_done) = (done.clone(), done.clone());
    let began = Instant::now();
    engine.channel().on_error(move |e| {
        let (ended, done, pid, outputs) =
            (on_error.clone(), error_done.clone(), e.pid.clone(), e.outputs.to_string());
        async move { record(&ended, &done, count, true, began, pid, outputs) }
    });
    engine.channel().on_complete(move |e| {
        let (ended, done, pid, outputs) = (
            on_complete.clone(),
            complete_done.clone(),
            e.pid.clone(),
            e.outputs.to_string(),
        );
        async move { record(&ended, &done, count, false, began, pid, outputs) }
    });

    for mid in mids {
        start(engine, principal, mid).await;
    }

    ended.recv().await
}

/// Record one finished run, and fire the signal once every run has reported.
fn record(
    ended: &Signal<Vec<Outcome>>,
    done: &AtomicUsize,
    count: usize,
    failed: bool,
    began: Instant,
    pid: String,
    outputs: String,
) {
    ended.update(|outcomes| {
        outcomes.push(Outcome {
            failed,
            elapsed: began.elapsed(),
            // `update` takes an `Fn`, so a captured value cannot be moved out
            // of the closure; it is cloned into the outcome instead.
            pid: pid.clone(),
            outputs: outputs.clone(),
        })
    });
    // The push precedes this count, so the `count`-th one closes a signal that
    // already holds every outcome.
    if done.fetch_add(1, Ordering::SeqCst) + 1 == count {
        ended.close();
    }
}

/// Poll `ready` every 20 ms until it holds or the deadline runs out — the
/// suite's wait convention: assertions poll for a state instead of assuming
/// how long reaching it takes.
pub async fn wait_for(secs: u64, mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..secs * 50 {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ready()
}
