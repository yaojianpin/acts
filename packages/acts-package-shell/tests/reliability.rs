//! Reliability of a shell act against the engine's own failure modes: an
//! engine that goes away mid-script must hand the run to the next engine
//! exactly once (restart), and a store that refuses the act's task-state
//! write must degrade without losing the run or its lane (store fault).
//!
//! Both points are proved on real runs over a shared store, the way a
//! deployment meets them: the script's side effect on the filesystem and the
//! durable rows the engine leaves behind are the evidence, not an event the
//! test itself staged.

mod support;

use acts::{
    ActError, Engine, KvStore, MemoryStore, Principal, ScanOptions, StoreBatchOp, Vars, Workflow,
    query::{Expr, Filter, Query},
};
use acts_package_shell::ShellPackage;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use support::{config, late_writer, quick, run_vars, scratch};

/// Workflow id of the single-step deployment every test here starts.
const MID: &str = "shell-reliability";

/// The shell and a script that writes `target` only once `gate` exists —
/// without the gate it blocks long enough for the test to take the engine
/// down first, so the file can only ever be written by a run that saw the
/// gate: the re-run on the restarted engine.
fn gated_writer(target: &Path, gate: &Path) -> (&'static str, String) {
    if cfg!(windows) {
        (
            "powershell",
            format!(
                "if (Test-Path '{}') {{ echo ok > '{}' }} else {{ Start-Sleep -Seconds 30 }}",
                gate.display(),
                target.display()
            ),
        )
    } else {
        (
            "sh",
            format!(
                "if [ -f '{}' ]; then echo ok > '{}'; else sleep 30; fi",
                gate.display(),
                target.display()
            ),
        )
    }
}

/// An engine on the caller's store backend with the shell package deployed.
async fn engine(store: Arc<dyn KvStore>, toml_text: &str) -> Engine {
    Engine::builder()
        .set_config(&config(toml_text))
        .set_store(store)
        .add_package::<ShellPackage>()
        .start()
        .await
        .unwrap()
}

/// Deploy one shell act and start it, returning the pid it runs under.
///
/// The start is retried a few times because a pid whose writer shard still
/// holds a failed-write ack from an earlier, already-handled fault is refused
/// once — the next flush caller is how the writer reports a store fault, and
/// a caller that treats the fault as transient retries.
async fn deploy_and_start(engine: &Engine, mid: &str, shell: &str, script: &str) -> String {
    let executor = engine.executor(&Principal::unrestricted());
    // The step is built from text rather than with a closure: `with_step`
    // takes a plain `fn`, and the script is a parameter of this test.
    let workflow = Workflow::from_yml(&format!(
        "name: shell run\nid: {mid}\nver: \"0.1.0\"\nsteps:\n  - id: s1\n    uses: acts.app.shell\n    params:\n      shell: {shell}\n      script: |\n        {script}\n"
    ))
    .unwrap();
    executor.model().deploy(&workflow, None).await.unwrap();

    let mut pid = None;
    for _ in 0..5 {
        match executor.proc().start(mid, Vars::new()).await {
            Ok(started) => {
                pid = Some(started);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    pid.expect("the run must start")
}

/// Poll `ready` every 20 ms until it holds or the deadline runs out — the
/// suite's wait convention: assertions poll for a state instead of assuming
/// how long reaching it takes.
async fn wait_for(secs: u64, mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..secs * 50 {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ready()
}

/// KV backend whose `batch` refuses the first write that persists a task row
/// while armed, and works normally otherwise: the store fault that swallows
/// exactly one task-state write of a running act. The fault is one-shot by
/// construction (`swap`), so the very next batch goes through whether the
/// test disarmed in between or not.
struct FlakyTaskWriteKv {
    inner: MemoryStore,
    armed: AtomicBool,
    fired: AtomicBool,
}

impl FlakyTaskWriteKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
            fired: AtomicBool::new(false),
        }
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    /// Whether the injected fault actually swallowed a task-row write.
    fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for FlakyTaskWriteKv {
    async fn one(&self, key: &str) -> acts::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> acts::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> acts::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> acts::Result<()> {
        // Only a `Put` of a task row is faulted: the completion write of a
        // finished act. Removals (`Delete`) of task rows are the process
        // sweep's cleanup — faulting those would strand a finished process
        // instead of exercising the act's own write path.
        let task_row = ops
            .iter()
            .any(|op| matches!(op, StoreBatchOp::Put { key, .. } if key.starts_with("tasks-id-")));
        if task_row && self.armed.swap(false, Ordering::SeqCst) {
            self.fired.store(true, Ordering::SeqCst);
            return Err(ActError::Store("injected: task-state write failed".into()));
        }
        self.inner.batch(ops).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> acts::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// An engine that goes away while a script blocks hands the run to the next
/// engine on the same store, and the run finishes exactly once: the shutdown
/// kills the blocked script instead of waiting for it, the interrupted leaf
/// task survives as recoverable state, the restarted engine resumes the
/// process as exactly one process row whose act re-runs the script to its end,
/// and nothing is left working behind it.
#[tokio::test]
async fn an_interrupted_script_resumes_once_after_an_engine_restart() {
    let dir = scratch("restart");
    let target = dir.join("done.txt");
    let gate = dir.join("gate");
    let (shell, script) = gated_writer(&target, &gate);

    // the shared store: the "crash" is engine teardown, the reload is a new
    // engine over the same backend
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());

    let engine1 = engine(store.clone(), "").await;
    let pid = deploy_and_start(&engine1, MID, shell, &script).await;
    let executor1 = engine1.executor(&Principal::unrestricted());
    let by_pid = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));

    // the act's own task row is durable before the script can have ended, so
    // the shutdown below finds a resumable run rather than a half-admitted one
    let mut rows = 0;
    for _ in 0..250 {
        rows = executor1.task().list(&by_pid).await.unwrap().rows.len();
        if rows >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        rows >= 2,
        "the root and the act task must be durable while the script runs"
    );
    assert_eq!(
        executor1
            .proc()
            .list(&Query::new())
            .await
            .unwrap()
            .rows
            .len(),
        1,
        "the run is exactly one process while it is in flight"
    );

    // shutdown with the script blocked: the engine must come back promptly —
    // a close that waits for a blocked script would hold the deployment hostage
    tokio::time::timeout(Duration::from_secs(30), engine1.close())
        .await
        .expect("close must not wait for a blocked script");

    // the interrupted run is recoverable state, not a lost one: the process
    // row is still in flight and its script never reached its side effect
    // (the gate did not exist, so even a script that survived the kill has
    // nothing to write)
    let crashed = executor1.proc().get(&pid).await.unwrap();
    assert_eq!(
        crashed.state, "running",
        "the interrupted run must stay in flight in the store"
    );
    assert!(
        !target.exists(),
        "the blocked script must not have completed its side effect"
    );

    // from here on the script's fast variant is armed: a run that sees the
    // gate writes the file and ends — only the re-run can be that run
    std::fs::write(&gate, b"go").unwrap();

    let engine2 = engine(store.clone(), "").await;
    let executor2 = engine2.executor(&Principal::unrestricted());

    // the resumed run drives itself to completion: its act re-runs the
    // script (gate present now), the file appears and the process settles.
    // A row that is gone was a completed one — completion is what arms the
    // sweeper, so absence is a settled answer, not a lost run.
    let mut completed = false;
    for _ in 0..1_500 {
        if target.exists() {
            match executor2.proc().get(&pid).await {
                Ok(info) => completed = info.state == "completed",
                Err(_) => completed = true,
            }
            if completed {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(completed, "the resumed run must complete on the new engine");
    assert!(
        target.exists(),
        "the re-run must have carried the script to its side effect"
    );

    // no duplicate process was created by the resume: the durable rows still
    // describe exactly one process, and it is this one
    let procs = executor2.proc().list(&Query::new()).await.unwrap().rows;
    assert_eq!(
        procs.len(),
        1,
        "the restart must not duplicate the process row"
    );
    assert_eq!(procs[0].id, pid);

    // nothing is left working: every task row of the run that remains is
    // settled — a stuck running leaf would mean the resume dropped work
    let mut stuck = Vec::new();
    for _ in 0..1_500 {
        stuck = executor2
            .task()
            .list(&by_pid)
            .await
            .unwrap()
            .rows
            .into_iter()
            .filter(|task| task.state == "running")
            .collect();
        if stuck.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        stuck.is_empty(),
        "no task of the resumed run may stay running"
    );

    engine2.close().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// A store that refuses the act's task-state write while the script runs
/// degrades instead of breaking the run: the engine neither hangs its lane
/// nor panics nor errors the process, the run completes exactly once once the
/// fault heals, and the engine takes ordinary work again right after — the
/// fault was one bad write, not a poisoned worker.
#[tokio::test]
async fn a_failed_task_state_write_degrades_without_losing_the_run() {
    let dir = scratch("store-fault");
    let target = dir.join("late.txt");
    let (shell, script) = late_writer(&target, 1);

    let kv = Arc::new(FlakyTaskWriteKv::new());
    let engine = engine(kv.clone(), "").await;

    // every terminal event of this engine is counted, so a completion the
    // flow reported twice cannot hide behind a signal that only fires once
    let completions = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let (c_ok, c_err) = (completions.clone(), errors.clone());
    engine.channel().on_complete(move |_| {
        c_ok.fetch_add(1, Ordering::SeqCst);
        async move {}
    });
    engine.channel().on_error(move |_| {
        c_err.fetch_add(1, Ordering::SeqCst);
        async move {}
    });

    let pid = deploy_and_start(&engine, MID, shell, &script).await;
    let executor = engine.executor(&Principal::unrestricted());
    let by_pid = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));

    // arm AFTER the launch, once the act's own task row is durable and the
    // script is the only thing left to happen: the next task-row write is the
    // act's completion, and that is the one the store refuses
    let mut rows = 0;
    for _ in 0..250 {
        rows = executor.task().list(&by_pid).await.unwrap().rows.len();
        if rows >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        rows >= 2,
        "the act task must be durable before the script ends"
    );
    kv.arm();

    // the script ends, the act's write hits the fault, and the run must still
    // reach its end — bounded, so a hung lane turns into a failure, not a
    // stuck suite
    let finished = wait_for(30, || completions.load(Ordering::SeqCst) >= 1).await;
    assert!(
        finished,
        "the run must complete despite the refused task-state write"
    );
    kv.disarm();

    assert!(
        kv.fired(),
        "the fault must have swallowed a task-row write, not sailed past"
    );

    // a duplicate completion report would double every downstream of the
    // event: settle briefly, then count what actually arrived
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        completions.load(Ordering::SeqCst),
        1,
        "the run must complete exactly once"
    );
    assert_eq!(
        errors.load(Ordering::SeqCst),
        0,
        "the store fault must not surface as an error of the run"
    );

    // the durable state agrees with the event: the process row reads
    // completed and no task of the run is left running. Read through the
    // plain row APIs — `proc().get` would first flush the pid's writer shard
    // and re-report the latched barrier failure of the injected fault, which
    // the engine surfaces by design on read-after-write
    let procs = executor.proc().list(&Query::new()).await.unwrap().rows;
    assert_eq!(procs.len(), 1, "exactly one process row for the run");
    assert_eq!(procs[0].id, pid);
    assert_eq!(
        procs[0].state, "completed",
        "the durable process must agree with the completion event"
    );
    for task in executor.task().list(&by_pid).await.unwrap().rows {
        assert_ne!(
            task.state, "running",
            "no task of a completed run may stay running"
        );
    }
    assert!(
        target.exists(),
        "the script's side effect must have survived the fault"
    );

    // the fault is over, and the engine is not: an ordinary act runs to
    // completion on the same worker that just carried the fault
    let (shell, script) = quick();
    let after = deploy_and_start(&engine, "shell-after-fault", shell, &script).await;
    let after_ok = wait_for(30, || completions.load(Ordering::SeqCst) >= 2).await;
    assert!(
        after_ok,
        "an act after the fault must complete on the released lane"
    );
    assert!(
        run_vars(&engine, &after).await.contains("ok"),
        "the act after the fault must have run and reported its output"
    );

    engine.close().await;
    std::fs::remove_dir_all(&dir).ok();
}
