//! Reliability of a shell act against the engine's own failure modes: an
//! engine that goes away while a script waits must come back without waiting
//! for it (and without that script finishing its work behind the shutdown),
//! and a store that refuses the act's task-state write must degrade without
//! losing the run or its lane (store fault).
//!
//! Both points are proved on real runs. What each reads back is the run's own
//! terminal event — the engine's report of it, output included — and the files
//! the script wrote in the directory it ran in while the act was still in
//! flight: the rows of a finished run are swept with that directory, and a read
//! of them races the writer that is still settling them.

mod support;

use acts::{
    ActError, KvStore, MemoryStore, Principal, ScanOptions, StoreBatchOp,
    query::{Expr, Filter, Query},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use support::{deploy, engine_on, quick, run_shell, scratch, start, wait_for, workdir};

/// Workflow id of the single-step deployment every test here starts.
const MID: &str = "shell-reliability";

/// The store-fault cell's script: it writes `late.txt` a second in and then
/// holds the act open for a while — the host reads the file the script wrote
/// while the act is still in flight, because the act's own completion is what
/// hands the run's directory to the sweeper.
fn held_writer() -> String {
    "sleep 1\necho late > late.txt\necho late\nsleep 3".to_string()
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

/// An engine that goes away while a script waits does not wait for it: the
/// shutdown stops the script instead, and the work the script had left never
/// happens.
///
/// What the run's own rows say afterwards is the engine's decision — a shutdown
/// can settle the interrupted act or leave it for the next engine to resume,
/// and a resumed run is dispatched while that engine is starting, before a test
/// can register the handler that would report its end — so this cell pins the
/// package's side of the contract: the close comes back without the script, and
/// the write the script was waiting to make never lands.
#[tokio::test]
async fn a_waiting_script_does_not_hold_the_shutdown() {
    let dir = scratch("shutdown");
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let (engine, principal) = engine_on(&dir, "", store).await;

    // `started.txt` is written before the wait, so the test sees the script
    // running while the run is still in flight; `late.txt` is what the wait was
    // going to do, and must never appear.
    let script = "echo started > started.txt\nsleep 3\necho late > late.txt";
    deploy(&engine, &principal, MID, script).await;
    let pid = start(&engine, &principal, MID).await;
    let run_dir = workdir(&dir, &pid);
    let running = wait_for(30, || run_dir.join("started.txt").exists()).await;
    assert!(
        running,
        "the script must be running, and its directory readable, before the engine closes"
    );

    let began = Instant::now();
    tokio::time::timeout(Duration::from_secs(30), engine.close())
        .await
        .expect("close must not wait for a script that is still waiting");
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "the close must come back, not wait for the script: took {:?}",
        began.elapsed()
    );

    // Outlive the wait the script never finished: a script that was only
    // abandoned would reach its write and leave the file behind.
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    let late = run_dir.join("late.txt");
    assert!(
        !late.exists(),
        "the stopped script wrote {} after the engine closed",
        late.display()
    );

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
    let kv = Arc::new(FlakyTaskWriteKv::new());
    let (engine, principal) = engine_on(&dir, "", kv.clone()).await;

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

    deploy(&engine, &principal, MID, &held_writer()).await;
    let pid = start(&engine, &principal, MID).await;
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

    // the script's side effect is read back while the act is still in flight:
    // the act completes after it wrote, so a file that is not there now is a
    // script that never reached its write
    let late = workdir(&dir, &pid).join("late.txt");
    let written = wait_for(10, || late.exists()).await;
    assert!(
        written,
        "the script must have written {} while the act runs",
        late.display()
    );
    assert_eq!(std::fs::read_to_string(&late).unwrap(), "late\n");

    // the run must still reach its end — bounded, so a hung lane turns into a
    // failure, not a stuck suite
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

    // the fault is over, and the engine is not: an ordinary act runs to
    // completion on the same worker that just carried the fault
    let after = run_shell(&engine, &principal, "shell-after-fault", &quick()).await;
    assert!(
        completions.load(Ordering::SeqCst) >= 2,
        "an act after the fault must complete on the released lane"
    );
    assert!(
        !after.failed && after.outputs.contains("ok"),
        "the act after the fault must have run and reported its output, got: {}",
        after.outputs
    );

    engine.close().await;
    std::fs::remove_dir_all(&dir).ok();
}
