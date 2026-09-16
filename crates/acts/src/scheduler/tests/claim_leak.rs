//! A failed launch must give its pid back.
//!
//! `Cache::admit` claims an externally supplied pid *before* any store I/O, so
//! a second start of the same pid fails deterministically. If the start then
//! fails — the first durable write of the process hits a transient store
//! fault — the claim is the only place that pid is still recorded: the
//! in-memory instance is evicted and the durable row was never written, so the
//! sweeper (which only removes rows that exist) never releases it. Every later
//! start of that pid then dies in admission with "proc_id(X) is duplicated in
//! running process list" although nothing runs and no row exists for X —
//! pointing troubleshooting at a phantom duplicate instead of the store fault
//! that really happened.

use crate::{
    ActError, Config, Vars, Workflow,
    config::ConfigData,
    scheduler::{Process, Runtime, TaskState},
    store::{KvStore, MemoryStore, ScanOptions, StoreBatchOp},
    utils::consts,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// KV backend that fails the first `batch` carrying a proc row and works
/// normally afterwards: the transient store fault of a launch whose very first
/// durable write never lands.
struct FailOnceProcBatchKv {
    inner: MemoryStore,
    armed: AtomicBool,
}

impl FailOnceProcBatchKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(true),
        }
    }
}

#[async_trait::async_trait]
impl KvStore for FailOnceProcBatchKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> crate::Result<()> {
        let proc_row = ops.iter().any(|op| match op {
            StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => {
                key.starts_with("procs-id-")
            }
        });
        if proc_row && self.armed.swap(false, Ordering::SeqCst) {
            return Err(ActError::Store("injected: backend unavailable".to_string()));
        }
        self.inner.batch(ops).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

#[tokio::test]
async fn failed_launch_releases_pid_claim() {
    let config = Config {
        data: ConfigData {
            cache_cap: Some(4),
            ..Default::default()
        },
        table: Default::default(),
    };
    let kv: Arc<dyn KvStore> = Arc::new(FailOnceProcBatchKv::new());
    let rt = Runtime::new(&config, Some(kv)).unwrap();
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let pid = "claimleak";
    let root = std::env::temp_dir().join(format!("acts_workdir_{}", crate::utils::longid()));
    let dir = root.join(pid);
    let vars = Vars::new().with(consts::PROCESS_ID, pid).with(
        consts::PROC_OWNER,
        crate::ScopePolicy {
            workdir_root: Some(root.clone()),
            ..Default::default()
        },
    );

    // the transient fault: the proc row write fails, so the start errors
    let err = rt.start(&workflow, vars.clone()).await.unwrap_err();
    assert!(matches!(err, ActError::Store(_)), "{err:?}");

    // ...and it left neither an instance nor a durable row behind
    assert!(
        rt.proc(pid).await.unwrap().is_none(),
        "no instance may survive a failed start"
    );
    assert!(
        !rt.cache().store().procs().exists(pid).await.unwrap(),
        "the failed start must not have written a row"
    );

    // ...and the directory the start created is gone with it: no row exists
    // that a sweep could ever find it through
    assert!(
        !dir.exists(),
        "a start that never became durable must leave no workdir behind"
    );

    // the pid is reusable: the retry is admitted instead of being rejected as
    // a duplicate of the start that failed
    let proc = rt
        .start(&workflow, vars)
        .await
        .expect("a failed start must release its pid claim");
    assert_eq!(proc.state(), TaskState::Running);
    assert!(
        dir.is_dir(),
        "the retry materializes the same workdir again"
    );

    rt.close().await;
    std::fs::remove_dir_all(&root).ok();
}

/// The complement of the case above: a *durable* row left behind by the failed
/// start (a backend may apply a batch only partially) keeps the pid claimed —
/// the row occupies its pid for the row's whole lifetime, and the `remove()`
/// that ends it is what releases the claim.
#[tokio::test]
async fn abandon_keeps_the_claim_of_a_durable_row() {
    let rt = Runtime::new(&Config::default(), None).unwrap();
    let cache = rt.cache();
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let pid = "abandondurable";
    let root = std::env::temp_dir().join(format!("acts_workdir_{}", crate::utils::longid()));
    let dir = root.join(pid);
    std::fs::create_dir_all(&dir).unwrap();
    let vars = Vars::new().with(consts::PROCESS_ID, pid);

    let proc = Process::new(pid, &rt);
    proc.load(&workflow).unwrap();
    proc.set_workdir(&dir);
    assert!(cache.admit(&proc).await.unwrap(), "the pid starts admitted");
    // the row the failed start would have left behind
    cache.store().upsert_proc(&proc).await.unwrap();

    cache.abandon(&proc).await;
    assert_eq!(cache.count(), 0, "the instance is evicted either way");
    assert!(
        dir.is_dir(),
        "a durable row keeps its workdir: the sweep that removes the row removes it"
    );

    // the claim survives with the row: a second start of the pid is refused
    let retry = rt.start(&workflow, vars.clone()).await.unwrap_err();
    assert!(
        retry
            .to_string()
            .contains("duplicated in running process list"),
        "a durable row keeps the pid occupied: {retry:?}"
    );
    // a *fresh* admission of that pid must fail too — the durable row, not a
    // lucky lookup, is what keeps it occupied
    let other = Process::new(pid, &rt);
    other.load(&workflow).unwrap();
    assert!(cache.admit(&other).await.is_err());

    // ...and removing the row is what gives the pid back
    cache.remove(pid).await.unwrap();
    assert!(
        !dir.exists(),
        "removing the row removes the workdir with it"
    );
    assert!(
        cache.admit(&other).await.unwrap(),
        "removing the row releases the pid claim"
    );

    rt.close().await;
    std::fs::remove_dir_all(&root).ok();
}
