mod health;
mod perf_probe;
mod scheduler_split;
mod state;
mod tree;

use super::*;
use crate::{
    scheduler::core::{
        commit::command::ActionKind,
        model::task_graph::TaskState,
        outbox::{
            outbox_dispatch::DeliveryOutcome,
            records::{OutboxRecord, OutboxState},
        },
    },
    store::MemoryStore,
};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How often one node may run in the harness's hosts: the configured default,
/// so a cell that loops on purpose is bounded exactly like a served run.
const MAX_NODE_RUN_TIMES: u64 = 1000;

/// The external id (`tid`) of one instance: the C5 wire of `node`'s
/// `generation` under `pid` — row keys, row `tid` columns and message `tid`s
/// all carry it; the model node stays on the row's `node_data.id` (`nid`).
fn tid_of(pid: &str, node: &str, generation: u64) -> String {
    crate::scheduler::core::model::task_graph::TaskId::new(node, generation)
        .unwrap()
        .wire(pid)
}

struct CountingTransport {
    delivered: Arc<parking_lot::Mutex<usize>>,
}

impl crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport for CountingTransport {
    fn deliver(&mut self, _record: &OutboxRecord) -> DeliveryOutcome {
        *self.delivered.lock() += 1;
        DeliveryOutcome::Delivered {
            receipt: "ok".into(),
        }
    }
}

/// Run the settle pass until it takes.
///
/// The driver's own tick runs the same pass, so a manual call can lose the
/// race to it and report the conflict instead of settling. A pass that keeps
/// failing is still a failure — this only absorbs the interleaving.
async fn settle_outbox(host: &KernelHost) {
    let mut last_error = None;
    for _ in 0..50 {
        match host.dispatch_outbox().await {
            Ok(_) => return,
            Err(error) => last_error = Some(error),
        }
    }
    panic!("the settle pass never took: {last_error:?}");
}

async fn assert_all_terminal(host: &KernelHost, pid: &str) {
    let snapshot = host.snapshot(pid).await.unwrap().expect("aggregate exists");
    assert!(!snapshot.tasks.is_empty());
    for (id, task) in &snapshot.tasks {
        assert!(
            task.state.is_terminal(),
            "execution {id:?} stuck at {:?}",
            task.state
        );
    }
    for (id, record) in &snapshot.operations {
        assert!(
            matches!(record.state, OperationState::Completed(_)),
            "operation {id:?} stuck at {:?}",
            record.state
        );
    }
}

/// No pure-internal hand-off survives a run: propagation, joins, the deadness
/// cascade and a rewind are folded into the commit that decided them (A3), so
/// the aggregate holds only the start, decision/effect and timer operations.
fn assert_no_folded_operations(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) {
    for (id, record) in &snapshot.operations {
        assert!(
            matches!(
                record.kind,
                crate::scheduler::core::model::operation::OperationKind::ExecuteTask
                    | crate::scheduler::core::model::operation::OperationKind::ApplyAction
                    | crate::scheduler::core::model::operation::OperationKind::DispatchTimeout
            ),
            "operation {id:?} is a folded hand-off: {:?}",
            record.kind
        );
    }
}

#[tokio::test]
async fn linear_process_runs_to_completion_and_delivers_outbox() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    // The transport is attached before the run: a hop delivers its own records
    // inside its commit, so nothing waits for a pass.
    let counter = Arc::new(parking_lot::Mutex::new(0));
    host.attach_outbox_transport(Box::new(CountingTransport {
        delivered: counter.clone(),
    }));
    let spec = ProcessSpec::linear("p1", "root", &["a", "b"]);
    host.start_process(&spec).await.unwrap();
    let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    // 3 nodes x (start + completion) operations. The propagation each
    // completion implies is folded into the completion's own commit, so the
    // run no longer claims a third operation per node: one claim and one plan
    // commit per surviving operation.
    assert_eq!(report.executed, 6, "six hops expected: {report:?}");
    assert_eq!(report.retried, 0);
    assert_all_terminal(&host, "p1").await;

    let snapshot = host.snapshot("p1").await.unwrap().unwrap();
    // No pure-internal operation survives the fold: the chain is the starts
    // and completions only.
    assert_no_folded_operations(&snapshot);
    // started x3 + completed x3 + propagated x3.
    assert_eq!(snapshot.outbox.len(), 9);

    // Every record went out with the commit that wrote it: the pass that runs
    // on the tick (and the retry net) has nothing left.
    assert_eq!(*counter.lock(), 9);
    let delivered = host.dispatch_outbox().await.unwrap();
    assert_eq!(delivered, 0, "in-hop delivery leaves no pending record");
    let snapshot = host.snapshot("p1").await.unwrap().unwrap();
    assert!(
        snapshot
            .outbox
            .values()
            .all(|record| matches!(record.state, OutboxState::Delivered { .. }))
    );
}

#[tokio::test]
async fn wake_loss_is_recovered_by_the_ready_queue() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::linear("p2", "root", &["only"]);
    // Create without waking: everything now rides on the durable ready
    // record, exactly like after a crash that lost the in-memory queue.
    let snapshot = spec.seed_snapshot().unwrap();
    host.processes
        .create(&snapshot, SystemClock.now_ms(), host.timer_poll_ms())
        .await
        .map_err(runtime)
        .unwrap();
    let now = SystemClock.now_ms();
    let report = host.tick(now).await.unwrap();
    assert!(
        report.reclaimed.is_empty() && report.ready_repaired.is_empty(),
        "the ready record was written with the aggregate: {report:?}"
    );
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert!(host.process_completed("p2").await.unwrap());
}

/// A store that parks a hop inside its *plan* commit and lets a test commit a
/// rival transition while it waits there.
///
/// The parked batch is the hop's own plan commit — the aggregate write that
/// follows its durable claim — and it is held *before* the guard is evaluated,
/// so the test's rival write lands in the window between the hop's `load` and
/// its compare-and-swap. The plan commit therefore loses its guard on demand
/// instead of by a race, which is what makes the regression below deterministic.
///
/// The batch is recognised by its content, not by counting: it writes
/// [`Self::aggregate`] while the hop's own operation is already durably
/// `CommitPrepared`, which is exactly the state the claim leaves behind and the
/// plan commit consumes.
struct ConflictKv {
    inner: MemoryStore,
    process_key: String,
    /// The operation whose plan commit should park. Tests that know the id up
    /// front arm it at construction; a test that discovers the operation after
    /// the chain starts arms it through [`Self::arm`].
    operation: parking_lot::Mutex<Option<OperationId>>,
    armed: AtomicBool,
    parks: AtomicUsize,
    parked: tokio::sync::Notify,
    released: parking_lot::Mutex<bool>,
    open: tokio::sync::Notify,
}

impl ConflictKv {
    /// A store ready to park one operation's plan commit in `process_id`'s
    /// aggregate. The operation is armed by [`Self::arm`].
    fn new(process_id: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            process_key: crate::scheduler::durable::document_store::process_key(process_id)
                .expect("a process id the aggregate key derives from"),
            operation: parking_lot::Mutex::new(None),
            armed: AtomicBool::new(true),
            parks: AtomicUsize::new(0),
            parked: tokio::sync::Notify::new(),
            released: parking_lot::Mutex::new(false),
            open: tokio::sync::Notify::new(),
        })
    }

    /// Arm the plan commit of `operation`. Must be called before that
    /// operation's claim lands.
    fn arm(&self, operation: OperationId) {
        *self.operation.lock() = Some(operation);
    }

    /// Whether this batch is the hop's plan commit: it writes the aggregate
    /// while the hop's own operation already stands `CommitPrepared`, i.e. the
    /// claim landed and the plan is about to.
    async fn is_plan_commit(&self, ops: &[crate::StoreBatchOp]) -> bool {
        let Some(operation) = self.operation.lock().clone() else {
            return false;
        };
        let writes_process = ops.iter().any(|op| match op {
            crate::StoreBatchOp::Put { key, .. } => key == &self.process_key,
            crate::StoreBatchOp::Delete { .. } => false,
        });
        if !writes_process {
            return false;
        }
        let Ok(Some(bytes)) = self.inner.one(&self.process_key).await else {
            return false;
        };
        let Ok(snapshot) =
            crate::scheduler::durable::document::ProcessDocumentCodec::decode(&bytes)
        else {
            return false;
        };
        matches!(
            snapshot
                .operations
                .get(&operation)
                .map(|record| &record.state),
            Some(OperationState::CommitPrepared(_))
        )
    }

    /// Hold this batch until [`Self::release`]. The arm is spent here: the
    /// rival writer's own batch must pass the gate.
    async fn hold(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.parks.fetch_add(1, Ordering::SeqCst);
        self.parked.notify_waiters();
        while !*self.released.lock() {
            let _ =
                tokio::time::timeout(std::time::Duration::from_millis(10), self.open.notified())
                    .await;
        }
    }

    /// Wait until the hop is parked inside its plan commit.
    async fn wait_parked(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.parks.load(Ordering::SeqCst) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the hop never reached its plan commit"
            );
            let _ =
                tokio::time::timeout(std::time::Duration::from_millis(10), self.parked.notified())
                    .await;
        }
    }

    /// Let the parked plan commit proceed (into the guard the rival has made
    /// stale).
    fn release(&self) {
        *self.released.lock() = true;
        self.open.notify_waiters();
    }
}

#[async_trait::async_trait]
impl KvStore for ConflictKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[crate::StoreBatchOp],
        guards: &[crate::StoreGuard],
    ) -> Result<bool> {
        if self.armed.load(Ordering::SeqCst) && self.is_plan_commit(ops).await {
            self.hold().await;
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: crate::ScanOptions,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// Counts compare-and-swap losses on one process's aggregate: every `batch`
/// that guards the process key and is refused. Under the command lane (A4) the
/// normal path produces none — each in-process writer of a pid holds its lane,
/// so a refused guard means two writers raced for the same revision.
struct GuardCountingKv {
    inner: MemoryStore,
    process_key: String,
    guard_misses: AtomicUsize,
}

impl GuardCountingKv {
    fn new(process_id: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            process_key: crate::scheduler::durable::document_store::process_key(process_id)
                .expect("a process id the aggregate key derives from"),
            guard_misses: AtomicUsize::new(0),
        })
    }

    fn guard_misses(&self) -> usize {
        self.guard_misses.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for GuardCountingKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[crate::StoreBatchOp],
        guards: &[crate::StoreGuard],
    ) -> Result<bool> {
        let applied = self.inner.batch(ops, guards).await?;
        if !applied && guards.iter().any(|guard| guard.key == self.process_key) {
            self.guard_misses.fetch_add(1, Ordering::SeqCst);
        }
        Ok(applied)
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: crate::ScanOptions,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

#[tokio::test]
async fn a_full_wake_queue_does_not_delay_the_ready_record() {
    let (host, _wake_rx) = KernelHost::in_memory();
    let dummy = OperationId {
        process_id: "queued-placeholder".into(),
        task: crate::scheduler::core::model::task_graph::TaskId::new("root", 0).unwrap(),
        sequence: 0,
    };
    while host.wake_tx.try_send(dummy.clone()).is_ok() {}
    let spec = ProcessSpec::linear("p-full", "root", &["only"]);
    host.start_process(&spec).await.unwrap();
    assert!(
        host.ready_metrics().wake_failures >= 1,
        "the start's wake must have been dropped by the full queue"
    );
    // No tick, no channel message: the durable record alone drives the run.
    let executed = host.drive_ready(SystemClock.now_ms(), 100).await.unwrap();
    assert!(executed > 0, "the due record must run without a wake");
    assert!(
        host.process_completed("p-full").await.unwrap(),
        "the ready queue carries the run to completion with a full wake channel"
    );
}

#[tokio::test]
async fn recovery_does_not_wake_a_parked_run() {
    let (host, _wake_rx) = KernelHost::in_memory_with_cap(0);
    let spec = ProcessSpec::linear("p-parked", "root", &["only"]);
    host.start_process(&spec).await.unwrap();
    let report = host.recover(SystemClock.now_ms()).await.unwrap();
    assert_eq!(
        report.scanned, 0,
        "a parked run is normal pending work, not recovery work: {report:?}"
    );
    assert!(report.reclaimed.is_empty() && report.ready_repaired.is_empty());
    assert_eq!(
        host.drive_ready(SystemClock.now_ms(), 8).await.unwrap(),
        0,
        "a zero-slot host may not drive the parked root"
    );
    let snapshot = host.snapshot("p-parked").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "root"),
        TaskState::Created,
        "the parked root stays undriven"
    );
    assert!(
        snapshot
            .operations
            .values()
            .all(|operation| matches!(operation.state, OperationState::Pending)),
        "the parked work keeps its durable ready record for admission"
    );
}

/// A hop whose *plan* commit loses its compare-and-swap must hand its claim
/// back to recovery — and recovery must be able to claim it again.
///
/// The plan commit is the commit that carries the hop's whole durable
/// remainder (the task's start, the act's effect, the propagation); when
/// another writer advances the aggregate between that hop's `load` and its
/// guard, the guard refuses and *nothing* is applied — so the hop's claim is
/// provably effect-free and must be released. This test forces exactly that
/// window with [`ConflictKv`]: it parks the hop inside its own plan commit and
/// commits a rival (a harmless marker variable) on the same aggregate, so the
/// hop's compare-and-swap is lost on demand rather than by a race — the
/// sibling-completion cell that found the defect only reproduces it
/// intermittently.
///
/// What it pins, and what either missing half of the fix did instead:
///
/// - the hop reports the conflict and the operation is `RetryScheduled`, the
///   state the recovery scan classifies `Wake`. Without the release the hop
///   returned the same conflict error but left the operation `CommitPrepared`:
///   its only recovery disposition is `AwaitOwner` until the claim's lease
///   expires, then `ReclaimExpired` — and because the conflict applied
///   nothing, the store cannot tell a commit that did not happen from one
///   whose external effect was interrupted, so the operation ended
///   `EffectUnknown` and its only disposition is `ManualReconciliation`. The
///   operation (and the run with it) was stranded for good, with nothing
///   drivable left for the engine.
/// - the recovery scan re-derives the released hop (`woken`, never
///   `reconciliation_required`).
/// - the re-drive claims cleanly and settles the run, the operation
///   `Completed`. A released claim can only be *re*-claimed by a fenced one:
///   the state machine requires `fence > attempt` to leave `RetryScheduled`, so
///   the driver must claim with the operation's own attempt + 1. Without that
///   the released operation was `RetryScheduled{attempt: 1}` and every hop
///   refused it with `InvalidOperationState`, stranding the run exactly as the
///   missing release did.
#[tokio::test]
async fn a_hop_that_loses_its_plan_commit_releases_the_claim_for_recovery() {
    const PID: &str = "p-conflict-release";
    let root = OperationId {
        process_id: PID.into(),
        task: crate::scheduler::core::model::task_graph::TaskId::new("root", 0).unwrap(),
        sequence: 0,
    };
    let stored = ConflictKv::new(PID);
    stored.arm(root.clone());
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let spec = ProcessSpec::linear(PID, "root", &["only"]);
    host.start_process(&spec).await.unwrap();

    // The root hop runs to its plan commit and parks there. The rival commits
    // on the same aggregate while the hop is between its load and its guard:
    // the hop's plan commit must then lose the aggregate's compare-and-swap.
    let hop = host.execute_one(&root);
    let rival = async {
        stored.wait_parked().await;
        let current = host.snapshot(PID).await.unwrap().expect("aggregate exists");
        assert!(
            matches!(
                current.operations.get(&root).map(|record| &record.state),
                Some(OperationState::CommitPrepared(_))
            ),
            "the parked batch is the hop's plan commit, not its claim: {:?}",
            current.operations.get(&root).map(|record| &record.state)
        );
        let plan = TransitionPlan {
            process_id: PID.into(),
            expected: current.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: vec![VariableWrite {
                scope: crate::scheduler::core::model::task_graph::TaskId::new("root", 0).unwrap(),
                name: "rival.marker".into(),
                value: "1".into(),
            }],
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        host.processes
            .commit_transition(&plan, SystemClock.now_ms(), host.timer_poll_ms())
            .await
            .unwrap();
        stored.release();
    };
    let (outcome, ()) = tokio::join!(hop, rival);
    let error = outcome.expect_err("a lost compare-and-swap must surface as an error");
    assert!(
        error.to_string().contains("commit conflict"),
        "the hop reports its own lost compare-and-swap: {error}"
    );

    // The pin: the claim went back, so the operation is drivable *now* — not
    // left `CommitPrepared` for the lease to expire into `EffectUnknown`.
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&root].state,
            OperationState::RetryScheduled { .. }
        ),
        "the conflicted hop must hand its claim back, not stay {:?}",
        snapshot.operations[&root].state
    );

    let report = host.tick(SystemClock.now_ms()).await.unwrap();
    assert!(
        !report.reclaimed.contains(&root),
        "the released hop is already in the ready index, not a recovery reclaim: {report:?}"
    );
    assert!(
        !report.reconciliation_required.contains(&root),
        "a lost compare-and-swap applied nothing, so it is not an unknown effect: {report:?}"
    );

    let driven = host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert_eq!(
        driven.retried, 0,
        "the released hop must claim cleanly on its next drive: {driven:?}"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&root].state,
            OperationState::Completed(_)
        ),
        "the re-driven hop completes its operation: {:?}",
        snapshot.operations[&root].state
    );
    assert!(
        host.process_completed(PID).await.unwrap(),
        "the re-driven hop must settle the run"
    );
}

/// A completion and the propagation it implies are one transition after A3:
/// the successor's `Created → Ready`, its `ExecuteTask` and the `propagate:`
/// record ride the completion's own commit. A lost compare-and-swap must
/// therefore apply *nothing* of them — no half-admitted successor, no
/// orphaned propagation record — and the effect-free completion goes back to
/// the retry queue, exactly like the pre-fold conflict path.
#[tokio::test]
async fn a_folded_completion_loses_its_commit_without_splitting_the_propagation() {
    const PID: &str = "p-fold-conflict";
    let root = crate::scheduler::core::model::task_graph::TaskId::new("root", 0).unwrap();
    let only = crate::scheduler::core::model::task_graph::TaskId::new("only", 0).unwrap();
    let start = OperationId {
        process_id: PID.into(),
        task: root.clone(),
        sequence: 0,
    };
    let completion = OperationId {
        process_id: PID.into(),
        task: root.clone(),
        sequence: 1,
    };
    let stored = ConflictKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let spec = ProcessSpec::linear(PID, "root", &["only"]);
    host.start_process(&spec).await.unwrap();
    // The start commits normally and seeds the completion; the completion is
    // the operation whose plan now also carries the propagation.
    assert!(host.execute_one(&start).await.unwrap());
    stored.arm(completion.clone());

    let hop_host = host.clone();
    let hop_operation = completion.clone();
    let hop = tokio::spawn(async move { hop_host.execute_one(&hop_operation).await });
    stored.wait_parked().await;
    let current = host.snapshot(PID).await.unwrap().expect("aggregate exists");
    let rival = TransitionPlan {
        process_id: PID.into(),
        expected: current.version.clone(),
        task_creates: Vec::new(),
        state_changes: Vec::new(),
        variable_writes: vec![VariableWrite {
            scope: root.clone(),
            name: "rival.marker".into(),
            value: "1".into(),
        }],
        source_operation: None,
        operations: Vec::new(),
        outbox: Vec::new(),
    };
    host.processes
        .commit_transition(&rival, SystemClock.now_ms(), host.timer_poll_ms())
        .await
        .unwrap();
    stored.release();
    hop.await
        .unwrap()
        .expect_err("a lost compare-and-swap must surface as a conflict");

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&completion].state,
            OperationState::RetryScheduled { .. }
        ),
        "the effect-free completion hands its claim back: {:?}",
        snapshot.operations[&completion].state
    );
    assert_eq!(
        snapshot.tasks[&only].state,
        crate::scheduler::core::model::task_graph::TaskState::Created,
        "a lost commit may not half-admit the successor"
    );
    let propagate = format!("propagate:{PID}:root:0:");
    assert_eq!(
        snapshot
            .outbox
            .keys()
            .filter(|id| id.key.starts_with(&propagate))
            .count(),
        0,
        "a lost commit may not leave the propagation record either"
    );

    let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert_eq!(
        report.retried, 0,
        "the released completion must claim cleanly on its next drive: {report:?}"
    );
    assert!(host.process_completed(PID).await.unwrap());
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        snapshot.tasks[&only].state,
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        snapshot
            .outbox
            .keys()
            .filter(|id| id.key.starts_with(&propagate))
            .count(),
        1,
        "the propagation lands exactly once on the retry"
    );
}

/// A package effect is an external boundary: once it has run, a lost plan
/// compare-and-swap must not release the claim for an automatic retry. The
/// operation goes to `EffectUnknown`, and recovery surfaces it for manual
/// reconciliation instead of re-driving it.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_package_effect_that_loses_its_commit_is_marked_unknown() {
    const PID: &str = "p-effect-conflict";
    let stored = ConflictKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let engine = crate::Engine::builder().start().await.unwrap();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

    let workflow = crate::model::Workflow::new()
        .with_id("wf-effect-conflict")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("n", 2),
            )
        });
    let spec = ProcessSpec::from_workflow(PID, &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();

    // Drive the chain until the package's `ApplyAction` is the next wake:
    // that is the operation whose plan commit crosses the effect boundary.
    let apply = loop {
        let id = wake_rx
            .try_recv()
            .expect("the chain must expose the package's ApplyAction operation");
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        let is_package_operation = id.task.node.as_str() == "s1"
            && snapshot.operations.get(&id).map(|record| record.kind)
                == Some(crate::scheduler::core::model::operation::OperationKind::ApplyAction);
        if is_package_operation {
            break id;
        }
        host.execute_one(&id).await.unwrap();
    };

    stored.arm(apply.clone());
    let hop_host = host.clone();
    let hop_operation = apply.clone();
    let hop = tokio::spawn(async move { hop_host.execute_one(&hop_operation).await });
    stored.wait_parked().await;

    // The package effect has already run. Advance the aggregate so the hop's
    // plan commit loses its guard.
    let current = host.snapshot(PID).await.unwrap().unwrap();
    let rival = TransitionPlan {
        process_id: PID.into(),
        expected: current.version.clone(),
        task_creates: Vec::new(),
        state_changes: Vec::new(),
        variable_writes: vec![VariableWrite {
            scope: crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap(),
            name: "rival.marker".into(),
            value: "1".into(),
        }],
        source_operation: None,
        operations: Vec::new(),
        outbox: Vec::new(),
    };
    host.processes
        .commit_transition(&rival, SystemClock.now_ms(), host.timer_poll_ms())
        .await
        .unwrap();
    stored.release();

    let error = hop
        .await
        .unwrap()
        .expect_err("the lost plan commit must surface");
    assert!(
        error.to_string().contains("conflict"),
        "the hop reports the lost compare-and-swap: {error}"
    );

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&apply].state,
            OperationState::EffectUnknown { .. }
        ),
        "an effect that may have run must not be retried automatically: {:?}",
        snapshot.operations[&apply].state
    );
    let report = host.tick(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.reconciliation_required.contains(&apply),
        "the unknown effect must be surfaced for manual reconciliation: {report:?}"
    );
    assert!(
        !report.reclaimed.contains(&apply),
        "an unknown effect is never re-queued as normal work: {report:?}"
    );
    engine.close().await;
}

// ---- A1c: effect idempotency and observation-driven reconciliation ----

/// One executor observation shared by a host and the test (and, for the
/// restart case, by a second host over the same store).
struct ObservedRunner {
    calls: Arc<AtomicUsize>,
    keys: Arc<parking_lot::Mutex<Vec<String>>>,
    observation: Arc<parking_lot::Mutex<effects::EffectObservation>>,
    /// Whether the runner publishes the applied observation. `false` models
    /// an executor that keeps none, which must leave the operation manual.
    record: bool,
    idempotent: bool,
}

#[async_trait::async_trait]
impl effects::EffectRunner for ObservedRunner {
    async fn resolve(
        &self,
        _request: &effects::EffectRequest,
    ) -> crate::Result<effects::ResolvedEffect> {
        Ok(effects::ResolvedEffect {
            params: serde_json::json!({ "n": 2 }),
            options: serde_json::Value::Null,
        })
    }

    async fn run(
        &self,
        _request: effects::EffectRequest,
        key: crate::scheduler::core::model::effect::EffectKey,
    ) -> crate::Result<effects::EffectOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.keys.lock().push(key.wire());
        let outcome =
            effects::EffectOutcome::from_outputs(crate::Vars::new().with("observed", "once"));
        if self.record {
            *self.observation.lock() = effects::EffectObservation::Applied(outcome.clone());
        }
        Ok(outcome)
    }

    fn idempotent(&self, _request: &effects::EffectRequest) -> bool {
        self.idempotent
    }

    async fn observe(
        &self,
        _key: &crate::scheduler::core::model::effect::EffectKey,
    ) -> crate::Result<effects::EffectObservation> {
        Ok(self.observation.lock().clone())
    }
}

/// An idempotent executor that deduplicates by the stable key: the host may
/// invoke it twice after a lost observation, but the external side effect
/// happens once.
struct DedupingRunner {
    attempts: Arc<AtomicUsize>,
    side_effects: Arc<AtomicUsize>,
    keys: Arc<parking_lot::Mutex<Vec<String>>>,
    seen: Arc<parking_lot::Mutex<std::collections::HashMap<String, effects::EffectOutcome>>>,
}

#[async_trait::async_trait]
impl effects::EffectRunner for DedupingRunner {
    async fn resolve(
        &self,
        _request: &effects::EffectRequest,
    ) -> crate::Result<effects::ResolvedEffect> {
        Ok(effects::ResolvedEffect {
            params: serde_json::json!({ "n": 2 }),
            options: serde_json::Value::Null,
        })
    }

    async fn run(
        &self,
        _request: effects::EffectRequest,
        key: crate::scheduler::core::model::effect::EffectKey,
    ) -> crate::Result<effects::EffectOutcome> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let wire = key.wire();
        self.keys.lock().push(wire.clone());
        if let Some(outcome) = self.seen.lock().get(&wire).cloned() {
            return Ok(outcome);
        }
        self.side_effects.fetch_add(1, Ordering::SeqCst);
        let outcome =
            effects::EffectOutcome::from_outputs(crate::Vars::new().with("observed", "once"));
        self.seen.lock().insert(wire, outcome.clone());
        Ok(outcome)
    }

    fn idempotent(&self, _request: &effects::EffectRequest) -> bool {
        true
    }

    async fn observe(
        &self,
        _key: &crate::scheduler::core::model::effect::EffectKey,
    ) -> crate::Result<effects::EffectObservation> {
        // The deliberately lost observation: the external duplicate would
        // have been real unless the executor deduplicates the key.
        Ok(effects::EffectObservation::NotApplied)
    }
}

/// A fresh host over `kv` with the same workflow shape the A1a conflict test
/// uses: one `uses` step whose `ApplyAction` crosses the effect boundary.
fn effect_kernel(
    kv: Arc<dyn KvStore>,
    runner: Arc<dyn effects::EffectRunner>,
) -> (Arc<KernelHost>, tokio::sync::mpsc::Receiver<OperationId>) {
    let (host, wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    host.attach_effect_runner(runner);
    (host, wake_rx)
}

async fn effect_host(
    kv: Arc<dyn KvStore>,
    pid: &str,
    runner: Arc<dyn effects::EffectRunner>,
) -> (
    Arc<KernelHost>,
    tokio::sync::mpsc::Receiver<OperationId>,
    crate::Engine,
) {
    let (host, wake_rx) = effect_kernel(kv, runner);
    // The step's `run_as` is resolved from the engine's package registry; the
    // effect itself never reaches the package (the test runner owns it).
    let engine = crate::Engine::builder().start().await.unwrap();
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let workflow = crate::model::Workflow::new()
        .with_id("wf-effect-reconcile")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("n", 2),
            )
        });
    let spec = ProcessSpec::from_workflow(pid, &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    (host, wake_rx, engine)
}

/// Drive the chain until the step's `ApplyAction` operation is the next wake.
async fn drive_to_step_effect(
    host: &KernelHost,
    wake_rx: &mut tokio::sync::mpsc::Receiver<OperationId>,
    pid: &str,
) -> OperationId {
    loop {
        let id = wake_rx
            .try_recv()
            .expect("the chain must expose the step's ApplyAction operation");
        let snapshot = host.snapshot(pid).await.unwrap().unwrap();
        let is_step_effect = id.task.node.as_str() == "s1"
            && snapshot.operations.get(&id).map(|record| record.kind)
                == Some(crate::scheduler::core::model::operation::OperationKind::ApplyAction);
        if is_step_effect {
            return id;
        }
        host.execute_one(&id).await.unwrap();
    }
}

/// Run `apply` to its plan commit and lose that commit: the effect has run,
/// the aggregate did not move.
async fn lose_the_effect_commit(
    host: &KernelHost,
    stored: &Arc<ConflictKv>,
    apply: &OperationId,
    pid: &str,
) {
    stored.arm(apply.clone());
    let hop_host = host.clone();
    let hop_operation = apply.clone();
    let hop = tokio::spawn(async move { hop_host.execute_one(&hop_operation).await });
    stored.wait_parked().await;

    let current = host.snapshot(pid).await.unwrap().unwrap();
    let rival = TransitionPlan {
        process_id: pid.into(),
        expected: current.version.clone(),
        task_creates: Vec::new(),
        state_changes: Vec::new(),
        variable_writes: vec![VariableWrite {
            scope: crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap(),
            name: "rival.marker".into(),
            value: "1".into(),
        }],
        source_operation: None,
        operations: Vec::new(),
        outbox: Vec::new(),
    };
    host.processes
        .commit_transition(&rival, SystemClock.now_ms(), host.timer_poll_ms())
        .await
        .unwrap();
    stored.release();

    let error = hop
        .await
        .unwrap()
        .expect_err("the lost plan commit must surface");
    assert!(
        error.to_string().contains("conflict"),
        "the hop reports the lost compare-and-swap: {error}"
    );
    let snapshot = host.snapshot(pid).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[apply].state,
            OperationState::EffectUnknown { .. }
        ),
        "an effect that may have run must not be retried automatically: {:?}",
        snapshot.operations[apply].state
    );
}

/// A1c: an executor observation that proves the effect ran converges the
/// operation without invoking the effect again. The effect count stays one,
/// and the run settles.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn an_observed_effect_converges_without_running_it_again() {
    const PID: &str = "p-effect-observed";
    let stored = ConflictKv::new(PID);
    let calls = Arc::new(AtomicUsize::new(0));
    let keys = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observation = Arc::new(parking_lot::Mutex::new(effects::EffectObservation::Unknown));
    let runner = Arc::new(ObservedRunner {
        calls: calls.clone(),
        keys: keys.clone(),
        observation,
        record: true,
        idempotent: false,
    });
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx, engine) = effect_host(kv, PID, runner).await;
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;

    lose_the_effect_commit(&host, &stored, &apply, PID).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the effect ran once");

    let report = host.tick(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.effect_applied.contains(&apply),
        "the applied observation must replay the operation: {report:?}"
    );
    assert!(
        !report.reconciliation_required.contains(&apply),
        "a proven-applied effect needs no human: {report:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "recovery may not call the effect again"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&apply].state,
            OperationState::Completed(_)
        ),
        "the operation converges to Completed: {:?}",
        snapshot.operations[&apply].state
    );
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(
        host.process_completed(PID).await.unwrap(),
        "the run settles from the observed outcome"
    );
    assert_eq!(keys.lock().len(), 1, "one effect key, one execution");
    engine.close().await;
}

/// A1c: observation `NotApplied` + a declared-idempotent effect re-queues the
/// operation; the executor deduplicates the stable key, so the external
/// side-effect count stays one while the operation runs twice.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn an_idempotent_effect_is_retried_under_the_same_key_and_dedupes() {
    const PID: &str = "p-effect-idempotent";
    let stored = ConflictKv::new(PID);
    let attempts = Arc::new(AtomicUsize::new(0));
    let side_effects = Arc::new(AtomicUsize::new(0));
    let keys = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let runner = Arc::new(DedupingRunner {
        attempts: attempts.clone(),
        side_effects: side_effects.clone(),
        keys: keys.clone(),
        seen: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
    });
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx, engine) = effect_host(kv, PID, runner).await;
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;

    lose_the_effect_commit(&host, &stored, &apply, PID).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let report = host.tick(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.effect_retried.contains(&apply),
        "not-applied + idempotent must re-queue: {report:?}"
    );
    assert!(
        !report.reconciliation_required.contains(&apply),
        "a provably safe retry needs no human: {report:?}"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&apply].state,
            OperationState::RetryScheduled { attempt: 1 }
        ),
        "the not-applied effect returns to the retry queue under its attempt: {:?}",
        snapshot.operations[&apply].state
    );

    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(
        host.process_completed(PID).await.unwrap(),
        "the retried effect settles the run"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "the operation ran twice"
    );
    assert_eq!(
        side_effects.load(Ordering::SeqCst),
        1,
        "the executor deduplicated the same key: one external effect"
    );
    engine.close().await;
    let keys = keys.lock();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1], "the retry reuses the stable effect key");
}

/// A1c: an effect with no observation and no idempotency declaration stays
/// manual across a restart. The operator resolves it `Applied`; the effect is
/// not called again and the decision is durable.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_non_idempotent_effect_is_manually_reconciled_across_restarts() {
    const PID: &str = "p-effect-manual";
    let stored = ConflictKv::new(PID);
    let calls = Arc::new(AtomicUsize::new(0));
    let keys = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observation = Arc::new(parking_lot::Mutex::new(effects::EffectObservation::Unknown));
    let runner = Arc::new(ObservedRunner {
        calls: calls.clone(),
        keys: keys.clone(),
        observation,
        record: false,
        idempotent: false,
    });
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx, engine) = effect_host(kv.clone(), PID, runner.clone()).await;
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;
    lose_the_effect_commit(&host, &stored, &apply, PID).await;

    let report = host.tick(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.reconciliation_required.contains(&apply),
        "an unprovable effect stays manual: {report:?}"
    );
    assert!(report.effect_applied.is_empty() && report.effect_retried.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // The same durable key is visible after a restart, before any decision.
    let (restarted, mut restarted_wake) = effect_kernel(kv.clone(), runner.clone());
    // A restart must not start the same pid again; the aggregate is loaded.
    assert!(restarted.exists(PID).await.unwrap());
    let unknowns = restarted.effect_unknowns(PID).await.unwrap();
    assert_eq!(unknowns.len(), 1, "one unknown effect: {unknowns:?}");
    assert_eq!(unknowns[0].sequence, apply.sequence);
    assert_eq!(
        unknowns[0].key,
        crate::scheduler::core::model::effect::EffectKey::of(&apply).wire(),
        "the key is rebuilt from the durable operation identity"
    );
    assert_eq!(
        unknowns[0].key,
        keys.lock().first().cloned().unwrap(),
        "the executor saw the same key the API exposes"
    );

    restarted
        .resolve_effect_by_key(
            PID,
            &unknowns[0].key,
            crate::EffectResolution::Applied {
                outputs: crate::Vars::new().with("manual", "applied"),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "manual applied resolution may not call the effect again"
    );
    restarted
        .run_until_quiescent(&mut restarted_wake, 200)
        .await
        .unwrap();
    assert!(
        restarted.process_completed(PID).await.unwrap(),
        "the manual applied decision settles the run"
    );
    assert!(
        restarted.effect_unknowns(PID).await.unwrap().is_empty(),
        "the durable decision clears the unknown state"
    );

    // A third host over the same store reads the committed outcome: the
    // decision is durable, not process-local.
    let (verified, _) = effect_kernel(kv.clone(), runner);
    let snapshot = verified.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&apply].state,
            OperationState::Completed(_)
        ),
        "the resolved operation stays Completed after a restart: {:?}",
        snapshot.operations[&apply].state
    );
    assert!(verified.effect_unknowns(PID).await.unwrap().is_empty());
    engine.close().await;
}

/// A1c: the operator may prove a non-idempotent effect never ran; the
/// operation returns to the retry queue and the effect runs again. This is a
/// human decision, not an automatic one.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_non_idempotent_effect_can_be_resolved_not_applied() {
    const PID: &str = "p-effect-not-applied";
    let stored = ConflictKv::new(PID);
    let calls = Arc::new(AtomicUsize::new(0));
    let keys = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observation = Arc::new(parking_lot::Mutex::new(effects::EffectObservation::Unknown));
    let runner = Arc::new(ObservedRunner {
        calls: calls.clone(),
        keys: keys.clone(),
        observation,
        record: false,
        idempotent: false,
    });
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx, engine) = effect_host(kv, PID, runner).await;
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;
    lose_the_effect_commit(&host, &stored, &apply, PID).await;

    let unknowns = host.effect_unknowns(PID).await.unwrap();
    host.resolve_effect_by_key(PID, &unknowns[0].key, crate::EffectResolution::NotApplied)
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(
        host.process_completed(PID).await.unwrap(),
        "a not-applied decision re-runs and settles the operation"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2, "the effect ran again");
    engine.close().await;
    let keys = keys.lock();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1], "the retry kept the stable key");
}

/// A8: an unknown effect retires the run from the execution quota immediately,
/// not when some later operation notices. With one execution slot, the parked
/// run behind it starts in the same deciding step — no tick, no recovery pass.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_quarantined_run_frees_its_slot_and_admits_the_parked_run() {
    const PID: &str = "p-quarantine-slot";
    const PARKED: &str = "p-quarantine-parked";
    let stored = ConflictKv::new(PID);
    let calls = Arc::new(AtomicUsize::new(0));
    let keys = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observation = Arc::new(parking_lot::Mutex::new(effects::EffectObservation::Unknown));
    let runner = Arc::new(ObservedRunner {
        calls: calls.clone(),
        keys,
        observation,
        record: false,
        idempotent: false,
    });
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx, engine) = effect_host(kv, PID, runner).await;
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;

    // The only slot belongs to the effect run; the next start parks.
    let spec = ProcessSpec::linear(PARKED, crate::utils::consts::TASK_ROOT_TID, &["only"]);
    host.start_process(&spec).await.unwrap();
    {
        let parked = host.snapshot(PARKED).await.unwrap().unwrap();
        assert!(
            parked
                .tasks
                .values()
                .all(|task| task.state == TaskState::Created),
            "the run behind the cap is parked"
        );
        assert_eq!(host.admission_metrics().parked, 1);
    }

    // The effect's commit is lost: `EffectUnknown` is durable. The run leaves
    // the execution quota and the parked run takes the slot in that step.
    lose_the_effect_commit(&host, &stored, &apply, PID).await;
    let metrics = host.admission_metrics();
    assert_eq!(metrics.execution.active, 1, "the parked run took the slot");
    assert_eq!(metrics.parked, 0, "the parked queue drained immediately");
    assert_eq!(metrics.quarantined_runs, 1);
    assert_eq!(
        metrics.reconciliation.active, 0,
        "no human decision is in flight"
    );

    host.run_until_quiescent(&mut wake_rx, 50).await.unwrap();
    assert!(
        host.process_completed(PARKED).await.unwrap(),
        "the admitted run progresses without a tick"
    );
    assert_eq!(
        host.effect_unknowns(PID).await.unwrap().len(),
        1,
        "the unknown effect still waits for its human decision"
    );
    engine.close().await;
}

/// A8: manual reconciliation is control-plane work with its own quota. It
/// commits while the only execution slot is held by another run, and that run
/// is neither parked nor delayed by it.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn manual_reconciliation_advances_while_the_execution_quota_is_full() {
    const PID: &str = "p-reconcile-full";
    const HOLDER: &str = "p-reconcile-holder";
    let stored = ConflictKv::new(PID);
    let calls = Arc::new(AtomicUsize::new(0));
    let keys = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observation = Arc::new(parking_lot::Mutex::new(effects::EffectObservation::Unknown));
    let runner = Arc::new(ObservedRunner {
        calls: calls.clone(),
        keys,
        observation,
        record: false,
        idempotent: false,
    });
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx, engine) = effect_host(kv, PID, runner).await;
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;
    lose_the_effect_commit(&host, &stored, &apply, PID).await;
    assert_eq!(host.admission_metrics().execution.active, 0);

    // A waiting act holds the only execution slot.
    let workflow = crate::model::Workflow::new()
        .with_id("wf-reconcile-holder")
        .with_step(|step| {
            step.with_id("hold").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "hold"),
            )
        });
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let spec = ProcessSpec::from_workflow(HOLDER, &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 50).await.unwrap();
    assert_eq!(
        state_of(&host.snapshot(HOLDER).await.unwrap().unwrap(), "hold"),
        TaskState::Waiting,
        "the holder waits on its act and occupies the slot"
    );
    let before = host.admission_metrics();
    assert_eq!(before.execution.active, 1);
    assert_eq!(before.parked, 0);

    // The human decision commits under the reconciliation quota only.
    let unknowns = host.effect_unknowns(PID).await.unwrap();
    host.resolve_effect_by_key(
        PID,
        &unknowns[0].key,
        crate::EffectResolution::Applied {
            outputs: crate::Vars::new().with("manual", "applied"),
        },
    )
    .await
    .unwrap();
    let after = host.admission_metrics();
    assert_eq!(
        after.execution.active, 1,
        "the holder keeps the only execution slot"
    );
    assert_eq!(
        after.quarantined_runs, 0,
        "the resolved run is no longer quarantined"
    );
    assert!(after.reconciliation.admitted >= 1);
    assert_eq!(after.reconciliation.active, 0, "the permit was returned");
    assert!(
        matches!(
            host.snapshot(PID).await.unwrap().unwrap().operations[&apply].state,
            OperationState::Completed(_)
        ),
        "the human decision committed"
    );

    // The holder still owns the slot; completing it lets the reconciled run
    // finish through the ordinary path.
    host.apply_action(
        HOLDER,
        &tid_of(HOLDER, "hold", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert!(host.process_completed(PID).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the effect never re-ran");
    engine.close().await;
}

/// A8: a client decision that ends the run returns the execution slot in the
/// deciding command, not on the next operation. The parked run is admitted by
/// that same `Decision`, so no tick is involved.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_cancel_frees_the_slot_and_admits_the_parked_run() {
    const PID: &str = "p-cancel-slot";
    const PARKED: &str = "p-cancel-parked";
    let (host, mut wake_rx) = KernelHost::in_memory_with_cap(1);
    let workflow = crate::model::Workflow::new()
        .with_id("wf-cancel-slot")
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    // Stop right before `s1`'s auto-completion: the slot is held and the
    // execution is still actionable.
    let _pending_auto = drive_to_step_effect(&host, &mut wake_rx, PID).await;

    let parked = ProcessSpec::linear(PARKED, crate::utils::consts::TASK_ROOT_TID, &["only"]);
    host.start_process(&parked).await.unwrap();
    assert_eq!(host.admission_metrics().parked, 1);

    host.apply_action(PID, &tid_of(PID, "s1", 0), ActionKind::Abort, Vec::new())
        .await
        .unwrap();
    assert!(
        host.process_completed(PID).await.unwrap(),
        "aborting the only live execution settles the run"
    );
    let metrics = host.admission_metrics();
    assert_eq!(metrics.execution.active, 1, "the parked run took the slot");
    assert_eq!(metrics.parked, 0);

    host.run_until_quiescent(&mut wake_rx, 50).await.unwrap();
    assert!(host.process_completed(PARKED).await.unwrap());
}

/// A4: concurrent client actions on one execution decide exactly once. Every
/// racer takes the pid's command lane in turn, so exactly one decision
/// commits on its first attempt and every loser is answered with the refusal
/// the moved state earned — never a compare-and-swap conflict, never a
/// re-plan. The instrumented store pins the stronger claim: not one aggregate
/// guard was refused, i.e. no two plans were ever formed from the same
/// revision.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_client_actions_decide_exactly_once_under_the_lane() {
    const PID: &str = "p-lane-clients";
    let stored = GuardCountingKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, usize::MAX, MAX_NODE_RUN_TIMES, 800);
    let workflow = crate::model::Workflow::new()
        .with_id("wf-lane-clients")
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    // Stop right before the step's auto-completion operation: `s1` is Running
    // and its decision is open to the clients below.
    let _pending_auto = drive_to_step_effect(&host, &mut wake_rx, PID).await;

    let racers: Vec<(ActionKind, Vec<(String, String)>)> = vec![
        (ActionKind::Complete, Vec::new()),
        (ActionKind::Complete, Vec::new()),
        (ActionKind::Complete, Vec::new()),
        (ActionKind::Complete, Vec::new()),
        (
            ActionKind::Fail {
                code: "E1".into(),
                message: "boom".into(),
            },
            Vec::new(),
        ),
        (
            ActionKind::Fail {
                code: "E2".into(),
                message: "boom".into(),
            },
            Vec::new(),
        ),
        // No forward package work to rewind (the refusal the legacy guard
        // gives), and no `to` to rewind to: both must be refused as domain
        // decisions, never as conflicts.
        (ActionKind::Cancel, Vec::new()),
        (ActionKind::Back, Vec::new()),
    ];
    let mut clients = Vec::new();
    for (kind, inputs) in racers {
        let host = host.clone();
        let pid = PID.to_string();
        let tid = tid_of(PID, "s1", 0);
        clients.push(tokio::spawn(async move {
            host.apply_action(&pid, &tid, kind, inputs).await
        }));
    }
    let mut successes = 0usize;
    for client in clients {
        match client.await.unwrap() {
            Ok(()) => successes += 1,
            Err(error) => assert!(
                !is_conflict(&error),
                "a loser must be refused as a decision, not as a conflict: {error}"
            ),
        }
    }
    assert_eq!(successes, 1, "exactly one decision commits");
    assert_eq!(
        host.action_replans(),
        0,
        "the lane serializes the racers: no re-plan on the normal path"
    );
    assert_eq!(
        stored.guard_misses(),
        0,
        "no two plans of this pid were formed from the same revision"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let decided = state_of(&snapshot, "s1");
    assert!(
        matches!(decided, TaskState::Completed | TaskState::Failed),
        "the winning decision decided the execution: {decided:?}"
    );
    let decisions = snapshot
        .operations
        .values()
        .filter(|record| record.id.task.node.as_str() == "s1")
        .filter(|record| {
            record.kind == crate::scheduler::core::model::operation::OperationKind::ApplyAction
        })
        .count();
    assert_eq!(
        decisions, 2,
        "the pending auto completion plus exactly one client decision"
    );
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert_all_terminal(&host, PID).await;
}

/// A4: the lane is per pid and no lane is held across an effect. Two runs'
/// step effects meet at a two-party barrier — impossible if either run's lane
/// (or one global driver lock) were held while the effect runs — and both
/// operations then commit.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn effects_of_different_runs_overlap_outside_the_lanes() {
    const A: &str = "p-lane-a";
    const B: &str = "p-lane-b";
    struct BarrierRunner {
        entered: Arc<AtomicUsize>,
        barrier: Arc<tokio::sync::Barrier>,
    }
    #[async_trait::async_trait]
    impl effects::EffectRunner for BarrierRunner {
        async fn resolve(
            &self,
            _request: &effects::EffectRequest,
        ) -> crate::Result<effects::ResolvedEffect> {
            Ok(effects::ResolvedEffect {
                params: serde_json::json!({ "n": 2 }),
                options: serde_json::Value::Null,
            })
        }

        async fn run(
            &self,
            _request: effects::EffectRequest,
            _key: crate::scheduler::core::model::effect::EffectKey,
        ) -> Result<effects::EffectOutcome> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            self.barrier.wait().await;
            Ok(effects::EffectOutcome::from_outputs(crate::Vars::new()))
        }
    }
    let kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let runner = Arc::new(BarrierRunner {
        entered: Arc::new(AtomicUsize::new(0)),
        barrier: Arc::new(tokio::sync::Barrier::new(2)),
    });
    // Two slots: both runs are in flight at once — the parallelism this test
    // measures (the shared effect harness caps at one run).
    let (host, mut wake_rx) = KernelHost::new(kv, 2, MAX_NODE_RUN_TIMES, 800);
    host.attach_effect_runner(runner.clone());
    let engine = crate::Engine::builder().start().await.unwrap();
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let workflow = crate::model::Workflow::new()
        .with_id("wf-lane-parallel")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("n", 2),
            )
        });
    for pid in [A, B] {
        let spec = ProcessSpec::from_workflow(pid, &workflow, &run_as).unwrap();
        host.start_process(&spec).await.unwrap();
    }
    // Drive everything except the two steps' effect operations.
    let mut applies = Vec::new();
    while applies.len() < 2 {
        let id = wake_rx
            .try_recv()
            .expect("both runs expose their step effect operation");
        let snapshot = host.snapshot(&id.process_id).await.unwrap().unwrap();
        let is_step_effect = id.task.node.as_str() == "s1"
            && snapshot.operations.get(&id).map(|record| record.kind)
                == Some(crate::scheduler::core::model::operation::OperationKind::ApplyAction);
        if is_step_effect {
            applies.push(id);
        } else {
            host.execute_one(&id).await.unwrap();
        }
    }
    let (first, second) = (host.clone(), host.clone());
    let (op_a, op_b) = (applies[0].clone(), applies[1].clone());
    let raced = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let a = tokio::spawn(async move { first.execute_one(&op_a).await });
        let b = tokio::spawn(async move { second.execute_one(&op_b).await });
        (a.await.unwrap(), b.await.unwrap())
    })
    .await
    .expect("the two effects must overlap: a lane held across an effect serializes runs");
    assert!(raced.0.unwrap() && raced.1.unwrap(), "both operations ran");
    assert_eq!(runner.entered.load(Ordering::SeqCst), 2);
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(host.process_completed(A).await.unwrap());
    assert!(host.process_completed(B).await.unwrap());
    engine.close().await;
}

/// A4: the lane is released for the effect window, and what that window costs
/// is exactly the A1a/A1c boundary. While the step's effect is in flight, a
/// client completes the same execution (impossible if the driver held the
/// lane); the effect's plan commit then loses its guard, the operation lands
/// `EffectUnknown`, recovery reports the manual reconciliation, and the
/// effect is never re-run.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn an_effect_releases_the_lane_for_a_concurrent_decision() {
    const PID: &str = "p-lane-effect-window";
    struct GateRunner {
        calls: Arc<AtomicUsize>,
        entered: tokio::sync::Notify,
        open: parking_lot::Mutex<bool>,
        released: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl effects::EffectRunner for GateRunner {
        async fn resolve(
            &self,
            _request: &effects::EffectRequest,
        ) -> crate::Result<effects::ResolvedEffect> {
            Ok(effects::ResolvedEffect {
                params: serde_json::json!({ "n": 2 }),
                options: serde_json::Value::Null,
            })
        }

        async fn run(
            &self,
            _request: effects::EffectRequest,
            _key: crate::scheduler::core::model::effect::EffectKey,
        ) -> Result<effects::EffectOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_waiters();
            while !*self.open.lock() {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(10),
                    self.released.notified(),
                )
                .await;
            }
            Ok(effects::EffectOutcome::from_outputs(crate::Vars::new()))
        }
    }
    let stored = GuardCountingKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let runner = Arc::new(GateRunner {
        calls: Arc::new(AtomicUsize::new(0)),
        entered: tokio::sync::Notify::new(),
        open: parking_lot::Mutex::new(false),
        released: tokio::sync::Notify::new(),
    });
    let (host, mut wake_rx) = effect_kernel(kv, runner.clone());
    let engine = crate::Engine::builder().start().await.unwrap();
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let workflow = crate::model::Workflow::new()
        .with_id("wf-lane-effect-window")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("n", 2),
            )
        });
    let spec = ProcessSpec::from_workflow(PID, &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    let apply = drive_to_step_effect(&host, &mut wake_rx, PID).await;
    let driving_host = host.clone();
    let driving = apply.clone();
    let driver = tokio::spawn(async move { driving_host.execute_one(&driving).await });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while runner.calls.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the effect never started"
        );
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            runner.entered.notified(),
        )
        .await;
    }

    // The same pid's client decision proceeds while the effect runs — the
    // proof the lane was handed back for the effect window.
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        host.apply_action(PID, &tid_of(PID, "s1", 0), ActionKind::Complete, Vec::new()),
    )
    .await
    .expect("a command of the same pid must proceed while its effect runs")
    .unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        TaskState::Completed,
        "the concurrent decision landed"
    );

    *runner.open.lock() = true;
    runner.released.notify_waiters();
    let error = driver
        .await
        .unwrap()
        .expect_err("the effect's late plan commit must lose its guard");
    assert!(
        error.to_string().contains("conflict"),
        "the driver reports the lost compare-and-swap: {error}"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        matches!(
            snapshot.operations[&apply].state,
            OperationState::EffectUnknown { .. }
        ),
        "an already-run effect is never retried automatically: {:?}",
        snapshot.operations[&apply].state
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "the effect ran exactly once"
    );

    // A1a/A1c semantics unchanged: recovery surfaces the manual
    // reconciliation, and the operator's resolve is itself a lane command —
    // it answers (here honestly refusing to replay a completion onto an
    // execution the client already completed) instead of deadlocking.
    let report = host.recover(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.reconciliation_required.contains(&apply),
        "the unknown effect needs a human: {report:?}"
    );
    let unknowns = host.effect_unknowns(PID).await.unwrap();
    assert_eq!(unknowns.len(), 1);
    let resolved = host
        .resolve_effect_by_key(
            PID,
            &unknowns[0].key,
            crate::EffectResolution::Applied {
                outputs: crate::Vars::new(),
            },
        )
        .await;
    assert!(
        resolved.is_err(),
        "the decision already completed the execution; a replayed completion cannot re-complete it"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "resolving never re-runs the effect"
    );
    engine.close().await;
}

/// The slot cap (`Config::cache_cap`): with one slot the first run owns it, the
/// runs started behind it park, and each freed slot admits the parked run that
/// has waited longest (`__created` ascending — the legacy parked row's
/// `timestamp` order). A parked run takes no hop at all and its root stays
/// `Created`, which the read model writes as the `None` process state: exactly
/// the shape of a legacy parked row.
#[tokio::test]
async fn a_run_beyond_the_cache_cap_parks_until_a_slot_frees() {
    let workflow = crate::model::Workflow::new()
        .with_id("wf-cap")
        .with_step(|step| step.with_id("s1"));
    let kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let (host, mut wake_rx) = KernelHost::new(kv.clone(), 1, MAX_NODE_RUN_TIMES, 800);
    // The read model is what a parked run is *visible* through, so this test
    // carries the projector the serving host attaches.
    let store = Arc::new(crate::store::Store::new(kv));
    let writer = crate::store::writebehind::StoreWriter::spawn(
        store.clone(),
        1,
        64,
        crate::utils::shared(1),
    );
    host.attach_row_projector(Arc::new(RowProjector::new(store.clone(), writer, None)));
    // Which runs the driver started, in order: the FIFO the cap must respect.
    let started = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let recorded = started.clone();
    host.attach_lifecycle_sink(Box::new(move |message, event| {
        if event == ProcessEvent::Started {
            recorded.lock().push(message.pid.clone());
        }
    }));

    for pid in ["cap-a", "cap-b", "cap-c"] {
        // Strictly increasing `__created` stamps: the parked order under test
        // is then the start order, not a millisecond tie.
        let previous = crate::utils::time::time_millis();
        while crate::utils::time::time_millis() == previous {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        let spec = ProcessSpec::from_workflow(pid, &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
    }

    // The only slot is `cap-a`'s: three of its four hops run here (the fourth
    // settles the run), and the two runs behind it never move. The propagation
    // is folded into each completion's commit, so a two-node run is four
    // operations, not six.
    let report = host.run_until_quiescent(&mut wake_rx, 3).await.unwrap();
    assert_eq!(
        report.executed, 3,
        "only the admitted run may hop: {report:?}"
    );
    assert_eq!(*started.lock(), vec!["cap-a".to_string()]);
    assert!(!host.process_completed("cap-a").await.unwrap());
    for pid in ["cap-b", "cap-c"] {
        assert_parked(&host, &store, pid).await;
    }

    // The remaining hop of `cap-a` settles it; the freed slot admits `cap-b`,
    // whose settlement admits `cap-c` — each one immediately, without waiting
    // for a tick.
    let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert_eq!(
        report.executed, 9,
        "one hop left of `cap-a` plus four each for the two admitted runs: {report:?}"
    );
    assert_eq!(
        *started.lock(),
        vec![
            "cap-a".to_string(),
            "cap-b".to_string(),
            "cap-c".to_string()
        ],
        "a freed slot admits the parked run that waited longest"
    );
    for pid in ["cap-a", "cap-b", "cap-c"] {
        assert!(host.process_completed(pid).await.unwrap(), "{pid} settled");
    }
}

/// The parked shape of one run: no hop at all (its single seeded operation is
/// still pending, nothing reached the outbox, every execution is `Created`),
/// and the read model's parked row — the process state `None` and a `Pending`
/// root task, what the legacy engine wrote for a process it could not start.
async fn assert_parked(host: &KernelHost, cache: &Arc<crate::store::Store>, pid: &str) {
    let snapshot = host.snapshot(pid).await.unwrap().expect("aggregate exists");
    assert_eq!(
        snapshot.operations.len(),
        1,
        "a parked run must not hop (nor be handed any other work)"
    );
    assert!(
        snapshot
            .operations
            .values()
            .all(|record| matches!(record.state, OperationState::Pending)),
        "the seeded operation waits for a slot"
    );
    assert!(snapshot.outbox.is_empty(), "a parked run writes no message");
    assert!(
        snapshot
            .tasks
            .values()
            .all(|task| task.state == TaskState::Created),
        "a parked run's root stays `Created`"
    );

    let store = cache.clone();
    let proc_row = store
        .procs()
        .find_opt(pid)
        .await
        .unwrap()
        .expect("proc row");
    assert_eq!(
        proc_row.state,
        crate::scheduler::core::model::legacy_task_state::LegacyTaskState::None.to_string(),
        "the parked process state, as the legacy row spells it"
    );
    let root_row = store
        .tasks()
        .find_opt(&crate::utils::Id::new(pid, &tid_of(pid, "$", 0)).id())
        .await
        .unwrap()
        .expect("root task row");
    assert_eq!(
        root_row.state,
        crate::scheduler::core::model::legacy_task_state::LegacyTaskState::Pending.to_string(),
        "the parked root task, as the legacy row spells it"
    );
}

/// A restart: the runs still in flight when the driver boots hold no slot yet,
/// and the recovery scan's first wake is where the cap meets them. The one over
/// the cap waits exactly like a start that found every slot taken — it takes no
/// hop at all — while the row it keeps is the one it had: a run that was in
/// flight is `Ready`/`Running`, not the `Created` of a parked start, which is
/// the shape of the legacy boot-resume overflow too.
#[tokio::test]
async fn a_run_already_in_flight_at_boot_waits_for_a_slot() {
    let (host, mut wake_rx) = KernelHost::in_memory_with_cap(1);
    let mut seeds = Vec::new();
    for pid in ["boot-a", "boot-b"] {
        // Seeded directly, as a crash leaves them: no driver ever admitted
        // them, so the boot pass is what places them. The root is the seeded
        // workflow node (`$`), which is what the shell's root facts key on.
        let spec = ProcessSpec::linear(pid, crate::utils::consts::TASK_ROOT_TID, &["only"]);
        let snapshot = spec.seed_snapshot().unwrap();
        host.processes
            .create(&snapshot, SystemClock.now_ms(), host.timer_poll_ms())
            .await
            .unwrap();
        seeds.push((pid.to_string(), snapshot));
    }
    let report = host.recover(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.reclaimed.is_empty() && report.ready_repaired.is_empty(),
        "a created run already owns its ready record: {report:?}"
    );
    // The ready index is the boot driver: it hands the first run its slot and
    // leaves the one behind the cap parked (with the record deferred).
    let now = SystemClock.now_ms();
    assert_eq!(host.drive_ready(now, 1).await.unwrap(), 1);
    let first = seeds[0].0.clone();
    let second = seeds[1].0.clone();

    // The run over the cap is offered work and waits.
    host.run_until_quiescent(&mut wake_rx, 2).await.unwrap();
    assert!(!host.process_completed(&first).await.unwrap());
    let waiting = host.snapshot(&second).await.unwrap().expect("aggregate");
    let seeded = &seeds
        .iter()
        .find(|(pid, _)| pid == &second)
        .expect("the second ready record belongs to a seeded run")
        .1;
    assert_eq!(
        waiting, *seeded,
        "a run waiting for a slot has not hopped: its aggregate is still its seed"
    );
    let _ = report;

    // The rest: the first run settles, and the slot it frees admits the one
    // that had been waiting.
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    for pid in ["boot-a", "boot-b"] {
        assert!(host.process_completed(pid).await.unwrap(), "{pid} settled");
    }
}

/// The flat-chain floor: the kernel host drives a flat workflow to the
/// expected terminal task set. Every remaining slice extends this harness
/// instead of asserting in isolation.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_path_runs_a_flat_chain_to_completion() {
    let workflow = {
        let mut workflow = crate::model::Workflow::new().with_id("wf-parity");
        workflow = workflow.with_step(|step| step.with_id("s1"));
        workflow = workflow.with_step(|step| step.with_id("s2"));
        workflow
    };
    let expected_tasks: Vec<String> = vec![
        crate::utils::consts::TASK_ROOT_TID.to_string(),
        "s1".into(),
        "s2".into(),
    ];

    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("k-parity", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert!(host.process_completed("k-parity").await.unwrap());
    let snapshot = host.snapshot("k-parity").await.unwrap().unwrap();
    let mut kernel_tasks: Vec<String> = snapshot
        .tasks
        .keys()
        .map(|id| id.node.as_str().to_owned())
        .collect();
    kernel_tasks.sort();
    assert_eq!(kernel_tasks, expected_tasks);
    assert!(snapshot.tasks.values().all(|e| e.state.is_terminal()));
}

/// The kernel engine, end to end: a start seeds the aggregate and no legacy
/// process runs, yet the engine's own completion event still fires — the
/// host reports the run's lifecycle — and every execution settles.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_serves_a_run_to_completion() {
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });

    let workflow = {
        let mut workflow = crate::model::Workflow::new().with_id("wf-driver");
        workflow = workflow.with_step(|step| step.with_id("s1"));
        workflow = workflow.with_step(|step| step.with_id("s2"));
        workflow
    };
    let proc = engine
        .runtime()
        .start(&workflow, crate::Vars::new())
        .await
        .unwrap();
    let pid = proc.id().to_string();
    tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
        .await
        .expect("engine never finished");

    // The run's durable record is the aggregate, and the legacy rows are its
    // projection (the read model this driver serves).
    let runtime = engine.runtime();
    let host = runtime.kernel();
    assert!(host.process_completed(&pid).await.unwrap());
    let snapshot = host.snapshot(&pid).await.unwrap().expect("aggregate");
    let mut tasks: Vec<String> = snapshot
        .tasks
        .keys()
        .map(|id| id.node.as_str().to_owned())
        .collect();
    tasks.sort();
    assert_eq!(
        tasks,
        vec![
            crate::utils::consts::TASK_ROOT_TID.to_string(),
            "s1".into(),
            "s2".into()
        ]
    );
    assert!(
        runtime.proc(&pid).await.unwrap().is_some(),
        "the projected rows serve the read API"
    );
}

/// The driver switch, action half: a waiting (Irq) act's Created message
/// comes from the kernel's outbox through the engine channel, and the
/// client's `act:complete` is served by the host — no legacy engine runs
/// on either side.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_serves_client_actions_on_an_irq_workflow() {
    use crate::event::EventAction;

    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let workflow = {
        let mut workflow = crate::model::Workflow::new().with_id("wf-irq");
        workflow = workflow.with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act1"),
            )
        });
        workflow = workflow.with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act2"),
            )
        });
        workflow
    };
    let runtime = engine.runtime();
    let host = runtime.kernel();

    // Subscribers attach BEFORE the start: the IRQ act's Created message
    // fires the moment the act runs, and a late subscriber misses it.
    let wait_for_created = |engine: &crate::Engine, key: &'static str| {
        let sig = engine.signal((String::new(), String::new()));
        let (done, done_rx) = (sig.clone(), sig.clone());
        let channel = engine.channel();
        channel.on_message(move |e| {
            let done = done_rx.clone();
            let key = key.to_string();
            async move {
                if e.is_params_key(&key) && e.is_state(crate::MessageState::Created) {
                    done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                    done.close();
                }
            }
        });
        done
    };
    let created1 = wait_for_created(&engine, "act1");
    let created2 = wait_for_created(&engine, "act2");

    let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
    let pid = proc.id().to_string();

    // Each waiting act completes exactly when its client action arrives.
    for created in [created1, created2] {
        let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
            .await
            .expect("waiting act never created")
            .clone();
        runtime
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .unwrap_or_else(|err| panic!("the kernel must serve act:complete: {err}"));
    }

    // The run settles once both acts are completed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if host.process_completed(&pid).await.unwrap_or(false) {
            break;
        }
        let states = host.snapshot(&pid).await.unwrap().map(|snapshot| {
            snapshot
                .tasks
                .iter()
                .map(|(id, task)| format!("{}:{:?}", id.node.as_str(), task.state))
                .collect::<Vec<_>>()
        });
        assert!(
            std::time::Instant::now() < deadline,
            "the run never settled: {states:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Driver mode, IO half: the effect's headless scope names the run's own
/// directory, so a filesystem package reads `$env.WORK_DIR` (and
/// `Context::workdir`) as `<workdir>/<pid>` — the same directory the
/// legacy start made.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_scopes_effects_to_the_run_workdir() {
    let root = std::env::temp_dir().join(format!("acts-kernel-wd-{}", crate::utils::longid()));
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    config.data.workdir = Some(root.display().to_string());
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });

    let workflow = crate::model::Workflow::new()
        .with_id("wf-workdir")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("dir", "${{ $env.WORK_DIR }}"),
            )
        });
    let proc = engine
        .runtime()
        .start(&workflow, crate::Vars::new())
        .await
        .unwrap();
    let pid = proc.id().to_string();
    tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
        .await
        .expect("engine never finished");

    let host = engine.runtime().kernel().clone();
    let snapshot = host.snapshot(&pid).await.unwrap().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    let dir = snapshot
        .variables
        .get(&(s1, "dir".to_string()))
        .expect("the effect output")
        .clone();
    assert_eq!(
        serde_json::from_str::<String>(&dir).unwrap(),
        root.join(&pid).display().to_string()
    );

    std::fs::remove_dir_all(&root).ok();
    engine.close().await;
}

/// The read model in driver mode: the kernel's rows land in the legacy
/// collections, so `proc:get`/`task:get`/`Runtime::proc` answer from them,
/// and once the run settled the sweeper reclaims its rows and its
/// directory.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_projects_the_legacy_read_model() {
    let root = std::env::temp_dir().join(format!("acts-kernel-rows-{}", crate::utils::longid()));
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    config.data.workdir = Some(root.display().to_string());
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    // An ack channel: its delivery rows only persist when the process's row
    // exists, so they are a direct probe of the projection (and of the
    // delivery close that lets a finished run be swept).
    let channel = engine.channel_with_options(&crate::ChannelOptions {
        ack: true,
        ..Default::default()
    });
    channel.on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    // The act messages only reach an ack channel that subscribes to them.
    channel.on_message(|_| async move {});

    let workflow = crate::model::Workflow::new()
        .with_id("wf-rows")
        .with_name("Rows")
        .with_step(|step| {
            step.with_name("Write").with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("k1", "v1"),
            )
        });
    let runtime = engine.runtime();
    let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
    let pid = proc.id().to_string();
    tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
        .await
        .expect("the run never finished");

    let executor = engine.executor(&crate::Principal::unrestricted());
    let info = executor.proc().get(&pid).await.expect("the proc row");
    assert_eq!(info.state, "completed");
    assert_eq!(info.mid, "wf-rows");
    let mut tasks: Vec<String> = info.tasks.iter().map(|task| task.id.clone()).collect();
    tasks.sort();
    let mut expected_tasks = vec![tid_of(&pid, "$", 0), tid_of(&pid, "s1", 0)];
    expected_tasks.sort();
    assert_eq!(tasks, expected_tasks);
    // The effect's output is the task scope's projected data.
    let s1 = executor
        .task()
        .get(&pid, &tid_of(&pid, "s1", 0))
        .await
        .expect("the task row");
    assert_eq!(s1.state, "completed");
    // The row contract: a `uses` step stays `step` even though its message is
    // an act (C6), and its name is the declaration's name.
    assert_eq!(s1.r#type, "step");
    assert_eq!(s1.name, "Write");
    let root_row = info
        .tasks
        .iter()
        .find(|task| task.id == tid_of(&pid, "$", 0))
        .expect("the root task row");
    assert_eq!(root_row.r#type, "workflow");
    assert_eq!(root_row.name, "Rows", "the root's row name is model.name");
    assert!(
        s1.data.contains("\"k1\":\"v1\""),
        "the task data must carry the effect output: {}",
        s1.data
    );
    // `Runtime::proc` rebuilds the process from the rows: the durable read
    // model, not a resident instance.
    let live = runtime
        .proc(&pid)
        .await
        .unwrap()
        .expect("the projected process must load");
    assert_eq!(live.tasks().len(), 2);

    // The run's own directory is what the effect used; it goes with the
    // rows once the deliveries settled (the outbox pass projects that) and
    // the sweeper runs.
    let dir = root.join(&pid);
    assert!(dir.is_dir(), "the run's directory must exist");
    let host = runtime.kernel().clone();
    host.dispatch_outbox().await.unwrap();
    // The message rows go through the write-behind queue: drain this
    // process's shard before reading the model, or a concurrent test's load
    // can leave the Created row still queued (the assertion below then sees
    // only the Completed one).
    runtime.flush_writer().await.unwrap();
    // The act message reached the engine's durable message rows (the ack
    // channel's delivery could only be stored once the process's row
    // existed), and the finished task settled it…
    let messages = executor
        .msg()
        .list(
            &crate::store::query::Query::new().filter(
                crate::store::query::Filter::and()
                    .expr(crate::store::query::Expr::eq("pid", pid.clone())),
            ),
        )
        .await
        .expect("the message rows");
    let mut act_states: Vec<String> = messages
        .rows
        .iter()
        .filter(|row| row.tid == tid_of(&pid, "s1", 0))
        .map(|row| row.state.to_string())
        .collect();
    act_states.sort();
    assert_eq!(
        act_states,
        vec!["completed".to_string(), "created".to_string()],
        "the act's Created and Completed messages must both have rows: {:?}",
        messages.rows
    );
    // The finished task settled every delivery of its messages, so the
    // run is sweepable. Re-derive the proc row first: the projection of the
    // final commit is asynchronous to the aggregate read above, and the sweep
    // selects terminal rows only.
    for row in &messages.rows {
        assert_eq!(
            row.status, "completed",
            "every delivery of a finished run must be settled: {:?}",
            messages.rows
        );
    }
    sweep_until_gone(&runtime, &pid).await;
    assert!(!dir.exists(), "the sweeper reclaims the run's directory");
    assert!(
        runtime
            .store()
            .procs()
            .find_opt(&pid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(runtime.proc(&pid).await.unwrap().is_none());
    // A11: the durable record and every derived index are gone too, so the
    // pid is reusable and nothing of the run survives as an index orphan.
    assert!(
        host.snapshot(&pid).await.unwrap().is_none(),
        "aggregate gone"
    );
    let kv = runtime.store().kv();
    for key in [
        crate::scheduler::durable::document_store::process_key(&pid).unwrap(),
        crate::scheduler::durable::document_store::recovery_index_key(&pid).unwrap(),
        crate::scheduler::durable::document_store::outbox_index_key(&pid).unwrap(),
        crate::scheduler::durable::document_store::ready_index_key(&pid).unwrap(),
    ] {
        assert!(
            kv.one(&key).await.unwrap().is_none(),
            "{key:?} must be reclaimed with the aggregate"
        );
    }

    std::fs::remove_dir_all(&root).ok();
    engine.close().await;
}

/// Restart discrimination (A11): a pid is refused while any durable trace of
/// the run remains (aggregate, rows, messages or deliveries), and becomes
/// reusable exactly once the cleanup removed the aggregate, the indexes, the
/// rows and the workdir.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_refuses_a_pid_until_the_run_is_reclaimed_and_then_reuses_it() {
    let config = crate::Config::default();
    // No periodic timers: the cleanup is driven explicitly, so the assertion
    // windows cannot race an automatic pass.
    let runtime = crate::scheduler::Runtime::new(&config, None).unwrap();
    let workflow = crate::model::Workflow::new()
        .with_id("wf-dup")
        .with_step(|step| step.with_id("s1"));
    let pid = "duppid";
    let options = crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid);
    runtime.start(&workflow, options.clone()).await.unwrap();
    wait_until_settled(&runtime, pid).await;

    let duplicated = runtime.start(&workflow, options.clone()).await;
    assert!(
        duplicated.is_err_and(|err| err.to_string().contains("duplicated")),
        "an existing pid must be refused while its rows are there"
    );

    // The run is still occupied even while only its rows are left: a start
    // races the cleanup, which deletes the aggregate before the rows.
    let host = runtime.kernel().clone();
    host.dispatch_outbox().await.unwrap();
    runtime.flush_writer().await.unwrap();
    assert!(
        host.exists(pid).await.unwrap(),
        "the aggregate is the record"
    );
    let duplicated = runtime.start(&workflow, options.clone()).await;
    assert!(
        duplicated.is_err_and(|err| err.to_string().contains("duplicated")),
        "an aggregate still present must refuse the pid"
    );

    // The cleanup reclaims the aggregate, the indexes, the rows and the
    // workdir; only then is the pid free.
    sweep_until_gone(&runtime, pid).await;
    assert!(
        runtime
            .store()
            .procs()
            .find_opt(pid)
            .await
            .unwrap()
            .is_none(),
        "the settled run's rows must be swept"
    );
    assert!(
        !host.exists(pid).await.unwrap(),
        "the aggregate must be reclaimed with the rows"
    );
    assert_no_row_families(&runtime.store(), pid).await;
    let reused = runtime
        .start(&workflow, options)
        .await
        .expect("a reclaimed pid must be reusable");
    assert_eq!(reused.id(), pid, "the new run keeps the requested pid");

    runtime.close().await;
}

/// Wait for a runtime-created run to settle (the runtime's driver owns the
/// operations; no engine timer is involved in these cleanup tests).
async fn wait_until_settled(runtime: &Arc<crate::scheduler::Runtime>, pid: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if runtime
            .kernel()
            .process_completed(pid)
            .await
            .unwrap_or(false)
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {pid} never settled"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Wait until the aggregate itself is reclaimable: every execution terminal,
/// every outbox record confirmed `Delivered`, every operation retired. Task
/// settlement alone is not enough under A1b — the confirmation commit follows
/// the plan commit — so cleanup tests wait on the A11 predicate.
async fn wait_until_reclaimable(host: &KernelHost, pid: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let _ = host.dispatch_outbox().await;
        if host
            .snapshot(pid)
            .await
            .unwrap_or(None)
            .is_some_and(|snapshot| snapshot.is_reclaimable())
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {pid} never became reclaimable: {:?}",
            host.snapshot(pid).await
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Drive the cleanup until `pid`'s rows are gone.
///
/// A single pass can legitimately find the run not yet reclaimable: the
/// final commit's projection is asynchronous, and a concurrent outbox pass can
/// hold a record `Claimed` for the instant the pass reads it. The production
/// timer retries; a test does the same with a short bound.
async fn sweep_until_gone(runtime: &Arc<crate::scheduler::Runtime>, pid: &str) {
    for _ in 0..200 {
        runtime.kernel().refresh_projection(pid).await;
        runtime.sweep_removable().await.unwrap();
        if runtime
            .store()
            .procs()
            .find_opt(pid)
            .await
            .unwrap()
            .is_none()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let snapshot = runtime.kernel().snapshot(pid).await.unwrap();
    panic!("run {pid} was not reclaimed: {snapshot:?}");
}

/// The row families the cleanup removes. `ops` is deliberately not checked:
/// it is the legacy outbox family L1 deletes, and pid existence never depends
/// on it.
async fn assert_no_row_families(store: &crate::store::Store, pid: &str) {
    use crate::store::query::{Expr, Filter, Query};

    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    assert!(
        store.procs().find_opt(pid).await.unwrap().is_none(),
        "proc row"
    );
    assert!(
        store.tasks().query(&q).await.unwrap().rows.is_empty(),
        "tasks"
    );
    assert!(
        store.vars().query(&q).await.unwrap().rows.is_empty(),
        "vars"
    );
    assert!(
        store.messages().query(&q).await.unwrap().rows.is_empty(),
        "messages"
    );
    assert!(
        store.deliveries().query(&q).await.unwrap().rows.is_empty(),
        "deliveries"
    );
}

/// A crash after the authoritative deletion batch (aggregate + indexes) but
/// before the row/workdir cleanup leaves the aggregate gone and the rows
/// behind. The next pass must finish the removal — and a further pass must be
/// a no-op.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_finishes_an_interrupted_terminal_cleanup() {
    let root = std::env::temp_dir().join(format!("acts-kernel-cut-{}", crate::utils::longid()));
    let mut config = crate::Config::default();
    config.data.workdir = Some(root.display().to_string());
    let runtime = crate::scheduler::Runtime::new(&config, None).unwrap();
    let pid = format!("interrupted_{}", crate::utils::longid());
    let workflow = crate::model::Workflow::new()
        .with_id("wf-cleanup")
        .with_step(|step| step.with_id("s1"));
    let options = crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid.clone());
    runtime.start(&workflow, options.clone()).await.unwrap();
    wait_until_settled(&runtime, &pid).await;
    let host = runtime.kernel().clone();
    wait_until_reclaimable(&host, &pid).await;
    runtime.flush_writer().await.unwrap();

    // The mark the cleanup writes before cutting, then the cut itself — the
    // crash happens before the rows and directory are removed.
    host.refresh_projection(&pid).await;
    let store = runtime.store();
    assert!(
        store
            .procs()
            .find_opt(&pid)
            .await
            .unwrap()
            .unwrap()
            .removable,
        "the run must be marked before the authoritative deletion"
    );
    let dir = root.join(&pid);
    assert!(dir.is_dir());
    let outcome = host.reclaim_process(&pid).await.unwrap();
    assert!(
        matches!(
            outcome,
            crate::scheduler::durable::async_document_store::ReclaimOutcome::Reclaimed { .. }
        ),
        "the authoritative batch must delete the aggregate: {outcome:?}"
    );
    assert!(!host.exists(&pid).await.unwrap());
    assert!(
        store.procs().find_opt(&pid).await.unwrap().is_some(),
        "the crash leaves the rows behind"
    );
    assert!(dir.is_dir(), "the crash leaves the directory behind");

    // The aggregate is gone but the residue is not: the pid exists by its
    // aggregate, yet the cleanup's per-pid mark keeps it closed until the
    // rows and workdir are removed.
    host.begin_reclaim(&pid);
    let blocked = runtime.start(&workflow, options.clone()).await;
    assert!(
        matches!(&blocked, Err(error) if error.to_string().contains("duplicated")),
        "a cleanup residue must keep the pid closed: {blocked:?}"
    );

    // The retry recognises the missing aggregate and finishes the cleanup.
    assert_eq!(runtime.sweep_removable().await.unwrap(), 1);
    assert_no_row_families(&store, &pid).await;
    assert!(!dir.exists());
    // Nothing left to do: a second pass is an idempotent no-op.
    assert_eq!(runtime.sweep_removable().await.unwrap(), 0);

    let reused = runtime
        .start(&workflow, options)
        .await
        .expect("the residue is gone, so the pid is reusable");
    assert_eq!(reused.id(), pid);

    std::fs::remove_dir_all(&root).ok();
    runtime.close().await;
}

/// An open (unsettled) delivery keeps the whole run — aggregate, indexes, rows
/// and workdir — alive; settling it lets the next pass reclaim everything.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_keeps_a_run_with_an_open_delivery() {
    use crate::store::data::{Delivery, DeliveryStatus};

    let root = std::env::temp_dir().join(format!("acts-kernel-open-{}", crate::utils::longid()));
    let mut config = crate::Config::default();
    config.data.workdir = Some(root.display().to_string());
    let runtime = crate::scheduler::Runtime::new(&config, None).unwrap();
    let pid = format!("open_{}", crate::utils::longid());
    let workflow = crate::model::Workflow::new()
        .with_id("wf-open")
        .with_step(|step| step.with_id("s1"));
    let options = crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid.clone());
    runtime.start(&workflow, options).await.unwrap();

    // A delivery row of this process that never settled: its task is not one
    // of the run's, so the task close cannot complete it.
    let store = runtime.store();
    let delivery_id = format!("open-delivery-{}", crate::utils::longid());
    let now = crate::utils::time::time_millis();
    store
        .deliveries()
        .create(&Delivery {
            id: delivery_id.clone(),
            msg_id: format!("open-message-{}", crate::utils::longid()),
            pid: pid.clone(),
            tid: "unrelated".to_string(),
            chan_id: "open-channel".to_string(),
            chan_pattern: "*".to_string(),
            status: DeliveryStatus::Created,
            retry_times: 0,
            create_time: now,
            update_time: now,
            timestamp: now,
            v: 1,
        })
        .await
        .unwrap();

    wait_until_settled(&runtime, &pid).await;
    let host = runtime.kernel().clone();
    wait_until_reclaimable(&host, &pid).await;
    runtime.flush_writer().await.unwrap();
    host.refresh_projection(&pid).await;
    let dir = root.join(&pid);
    assert!(dir.is_dir());

    runtime.sweep_removable().await.unwrap();
    assert!(
        host.exists(&pid).await.unwrap(),
        "an open delivery must keep the aggregate"
    );
    assert!(store.procs().find_opt(&pid).await.unwrap().is_some());
    assert!(dir.is_dir(), "an open delivery must keep the workdir");

    store
        .set_delivery(&delivery_id, DeliveryStatus::Completed)
        .await
        .unwrap();
    assert_eq!(
        store.deliveries().find(&delivery_id).await.unwrap().status,
        DeliveryStatus::Completed
    );
    assert!(!store.has_unsettled_deliveries(&pid).await.unwrap());
    let snapshot = host.snapshot(&pid).await.unwrap().expect("the aggregate");
    assert!(
        snapshot.is_reclaimable(),
        "operations={:?} outbox={:?} tasks={:?}",
        snapshot
            .operations
            .values()
            .map(|op| &op.state)
            .collect::<Vec<_>>(),
        snapshot
            .outbox
            .values()
            .map(|record| &record.state)
            .collect::<Vec<_>>(),
        snapshot
            .tasks
            .values()
            .map(|task| task.state)
            .collect::<Vec<_>>(),
    );
    sweep_until_gone(&runtime, &pid).await;
    assert!(!host.exists(&pid).await.unwrap());
    assert!(store.procs().find_opt(&pid).await.unwrap().is_none());
    assert!(!dir.exists());

    std::fs::remove_dir_all(&root).ok();
    runtime.close().await;
}

/// The package write-back: what a Func package leaves in its context —
/// the returned outputs, the task data it wrote, the process env it
/// set — commits with the completion, the env on the run's root scope,
/// where a later task's `$env` reads it and the projected proc row carries
/// it.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn effect_write_back_folds_into_the_process() {
    struct WriteBack;
    #[async_trait::async_trait]
    impl crate::package::ActPackage for WriteBack {
        fn new(_: &crate::Config) -> crate::Result<Self> {
            Ok(Self)
        }
        fn definition() -> crate::package::ActPackageDefinition {
            crate::package::ActPackageDefinition {
                id: "test.writeback",
                name: "WriteBack",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: serde_json::json!({}),
                options: None,
                run_as: crate::package::ActRunAs::Func,
                resources: Vec::new(),
                catalog: crate::package::ActPackageCatalog::App,
            }
        }
        async fn execute(
            &self,
            ctx: &crate::Context,
            _params: &serde_json::Value,
        ) -> crate::Result<Option<crate::Vars>> {
            ctx.set_env("GREETING", "hi");
            // A1c: the package sees the stable operation-derived key, and the
            // private carrier never becomes durable run data (asserted below).
            let effect_key = ctx
                .effect_key()
                .expect("a package effect carries its stable key");
            ctx.task().update_data(
                &crate::Vars::new()
                    .with("written", 7)
                    .with("effect_key", effect_key),
            );
            Ok(Some(crate::Vars::new().with("output", 1)))
        }
    }
    struct ReadEnv;
    #[async_trait::async_trait]
    impl crate::package::ActPackage for ReadEnv {
        fn new(_: &crate::Config) -> crate::Result<Self> {
            Ok(Self)
        }
        fn definition() -> crate::package::ActPackageDefinition {
            crate::package::ActPackageDefinition {
                id: "test.readenv",
                name: "ReadEnv",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: serde_json::json!({}),
                options: None,
                run_as: crate::package::ActRunAs::Func,
                resources: Vec::new(),
                catalog: crate::package::ActPackageCatalog::App,
            }
        }
        async fn execute(
            &self,
            ctx: &crate::Context,
            _params: &serde_json::Value,
        ) -> crate::Result<Option<crate::Vars>> {
            Ok(Some(
                crate::Vars::new().with("seen", ctx.get_env::<String>("GREETING")),
            ))
        }
    }

    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let runtime = engine.runtime();
    let package = runtime.package().clone();
    // The production path: the durable metadata row (what the effect
    // runner resolves `run_as`/params against) plus the instance factory.
    let executor = engine.executor(&crate::Principal::unrestricted());
    executor
        .ext()
        .register_package(&<WriteBack as crate::package::ActPackage>::definition())
        .await
        .unwrap();
    executor
        .ext()
        .register_package(&<ReadEnv as crate::package::ActPackage>::definition())
        .await
        .unwrap();
    package.register(
        "test.writeback",
        &crate::package::ActPackageRegister::new::<WriteBack>(),
    );
    package.register(
        "test.readenv",
        &crate::package::ActPackageRegister::new::<ReadEnv>(),
    );
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });

    let workflow = crate::model::Workflow::new()
        .with_id("wf-writeback")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses("test.writeback", crate::Vars::new())
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses("test.readenv", crate::Vars::new())
        });
    let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
    let pid = proc.id().to_string();
    tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
        .await
        .expect("the run never finished");

    let host = runtime.kernel().clone();
    let snapshot = host.snapshot(&pid).await.unwrap().unwrap();
    let var = |task: &str, name: &str| {
        snapshot
            .variables
            .get(&(
                crate::scheduler::core::model::task_graph::TaskId::new(task, 0).unwrap(),
                name.to_string(),
            ))
            .cloned()
    };
    assert_eq!(var("s1", "output").as_deref(), Some("1"));
    assert_eq!(var("s1", "written").as_deref(), Some("7"));
    // The package saw the key of the ApplyAction operation that ran it; the
    // key is the durable operation identity, not a random per-call token.
    let apply = snapshot
        .operations
        .values()
        .find(|record| {
            record.kind == crate::scheduler::core::model::operation::OperationKind::ApplyAction
                && record.id.task.node.as_str() == "s1"
        })
        .expect("the step's effect operation");
    let expected = format!(
        "\"{}\"",
        crate::scheduler::core::model::effect::EffectKey::of(&apply.id).wire()
    );
    assert_eq!(var("s1", "effect_key").as_deref(), Some(expected.as_str()));
    // The env the package set is the run's, read back by the next task.
    assert_eq!(var("s2", "seen").as_deref(), Some("\"hi\""));
    // …and it is projected into the run's row, exactly like the legacy
    // process env.
    let row = runtime
        .store()
        .procs()
        .find(&pid)
        .await
        .expect("the proc row");
    assert!(
        row.env.contains("\"GREETING\":\"hi\""),
        "the proc env must carry the package env: {}",
        row.env
    );
    assert!(
        !row.env.contains(crate::utils::consts::EFFECT_KEY),
        "the effect key is a private carrier, not durable run data: {}",
        row.env
    );

    engine.close().await;
}

/// The kernel's message stream for one workflow: per-task messages folded
/// onto their owning node (an act folds into its step; the workflow root is
/// `$`), the process lifecycle events, and the read-model rows.
///
/// The workflow carries a package-less step beside the acts: the kernel
/// announces every task it creates and settles, and a container's own
/// completion, not only the acts.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_task_and_act_messages_are_emitted() {
    use crate::event::EventAction;

    // Returns (folded task messages, lifecycle events seen, folded read-model
    // rows). The lifecycle is the process-level channel
    // (`on_start`/`on_complete`), where the kernel reports the workflow root;
    // the rows are what `proc:get`/`task:get` read (an act row folds to its
    // owning step's node).
    async fn run() -> (Vec<String>, bool, bool, Vec<String>, bool) {
        let workflow = crate::model::Workflow::new()
            .with_id("wf-msg-parity")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_SET,
                    crate::Vars::new().with("k1", "v1"),
                )
            })
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "irq-1"),
                )
            })
            .with_step(|step| step.with_id("s3"));
        let mut config = crate::Config::default();
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sig = engine.signal(());
        let done = sig.clone();
        {
            let completed = completed.clone();
            engine.channel().on_complete(move |_| {
                let completed = completed.clone();
                let done = done.clone();
                async move {
                    completed.store(true, std::sync::atomic::Ordering::SeqCst);
                    done.close();
                }
            });
        }
        {
            let started = started.clone();
            engine.channel().on_start(move |_| {
                let started = started.clone();
                async move {
                    started.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }
        let folded: Arc<parking_lot::Mutex<Vec<String>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let saw_workflow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = engine.runtime();
        {
            let folded = folded.clone();
            let saw_workflow = saw_workflow.clone();
            engine.channel().on_message(move |e| {
                let folded = folded.clone();
                let saw_workflow = saw_workflow.clone();
                async move {
                    let uses = e.uses.clone().unwrap_or_default();
                    // The workflow root's task messages are the process
                    // lifecycle, which the kernel reports on the lifecycle
                    // channel *and* — like the legacy engine — as a task
                    // message on this one; the fold counts their arrival but
                    // compares only act/step messages (asserted below).
                    if e.r#type == "workflow" {
                        saw_workflow.store(true, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                    // Fold onto the owning node: an act belongs to its step
                    // (the kernel's node identity), everything else is its
                    // own node.
                    let node = if e.r#type == "act" {
                        e.inputs
                            .get::<crate::Vars>(crate::utils::consts::STEP_KEY)
                            .and_then(|step| step.get::<String>(crate::utils::consts::STEP_NODE_ID))
                            .unwrap_or_else(|| e.nid.clone())
                    } else {
                        e.nid.clone()
                    };
                    let state = e.state.to_string();
                    folded
                        .lock()
                        .push(format!("{node}|{state}|{}", !uses.is_empty()));
                }
            });
        }
        let created = engine.signal((String::new(), String::new()));
        {
            let created = created.clone();
            engine.channel().on_message(move |e| {
                let created = created.clone();
                async move {
                    if e.r#type == "act"
                        && e.is_params_key("irq-1")
                        && e.state == crate::MessageState::Created
                    {
                        created.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        created.close();
                    }
                }
            });
        }

        let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
        let _ = proc.id();
        // The client's action completes the waiting act, on the tid its
        // own Created message named (the legacy act's random task id, the
        // kernel's node id).
        let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
            .await
            .expect("the waiting act was never created")
            .clone();
        runtime
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the waiting act must complete");
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("the run never finished");
        // The driver delivers records on its tick; drain them so the
        // comparison sees the same settled stream the legacy engine
        // emitted synchronously.
        runtime.kernel().dispatch_outbox().await.unwrap();
        // The rows are a write-behind projection: they lag the completion
        // event by a queue turn, and a projection landing after `close` is
        // refused (the rows would then read the state before it). Wait for
        // both the proc row and the root task row it summarizes: the proc row
        // can be projected from a later commit than the root task row, so
        // checking only the proc row can read a still-running root.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let query = crate::store::query::Query::new().filter(
            crate::store::query::Filter::and()
                .expr(crate::store::query::Expr::eq("pid", pid.clone())),
        );
        loop {
            runtime.flush_writer().await.unwrap();
            let state = runtime
                .store()
                .procs()
                .find_opt(&pid)
                .await
                .unwrap()
                .map(|row| row.state);
            let root_terminal = runtime
                .store()
                .tasks()
                .query_all(&query)
                .await
                .unwrap()
                .iter()
                .any(|row| row.kind == "workflow" && row.state != "running" && row.state != "none");
            if state
                .as_deref()
                .is_some_and(|state| state != "running" && state != "none")
                && root_terminal
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run's rows never read terminal: proc={state:?} root={root_terminal}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // The durable read model of this run, folded: a row's node is its
        // own `node_data` id (the workflow root is `$`, as in the kernel),
        // and an act row folds to the node of its parent row. The legacy
        // writer is drained first: its rows lag the completion signal.
        runtime.flush_writer().await.unwrap();
        let rows_raw = runtime
            .store()
            .tasks()
            .query_all(
                &crate::store::query::Query::new().filter(
                    crate::store::query::Filter::and()
                        .expr(crate::store::query::Expr::eq("pid", pid.clone())),
                ),
            )
            .await
            .unwrap();
        let node_id_of = |tid: &str| {
            rows_raw
                .iter()
                .find(|row| row.tid == tid)
                .and_then(|row| {
                    serde_json::from_str::<crate::scheduler::core::model::view::NodeData>(
                        &row.node_data,
                    )
                    .ok()
                })
                .map(|data| data.id)
        };
        let mut rows: Vec<String> = Vec::new();
        for row in &rows_raw {
            let node = if row.kind == "workflow" {
                crate::utils::consts::TASK_ROOT_TID.to_string()
            } else if row.kind == "act" {
                row.parent
                    .as_deref()
                    .and_then(node_id_of)
                    .unwrap_or_else(|| row.tid.clone())
            } else {
                node_id_of(&row.tid).unwrap_or_else(|| row.tid.clone())
            };
            rows.push(format!("{node}|{}", row.state));
        }
        rows.sort();
        rows.dedup();
        let mut folded = folded.lock().clone();
        folded.sort();
        folded.dedup();
        engine.close().await;
        (
            folded,
            started.load(std::sync::atomic::Ordering::SeqCst),
            completed.load(std::sync::atomic::Ordering::SeqCst),
            rows,
            saw_workflow.load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    let (kernel, kernel_started, kernel_completed, kernel_rows, kernel_workflow) = run().await;
    // Each step speaks its `created` and `completed`; the two `uses` steps
    // carry their act (`true`), the package-less one does not.
    assert_eq!(
        kernel,
        vec![
            "s1|completed|true".to_string(),
            "s1|created|true".to_string(),
            "s2|completed|true".to_string(),
            "s2|created|true".to_string(),
            "s3|completed|false".to_string(),
            "s3|created|false".to_string(),
        ],
        "the folded task message set"
    );
    // The read model: one row per node, all settled (the act folds into its
    // step).
    assert_eq!(
        kernel_rows,
        vec![
            "$|completed".to_string(),
            "s1|completed".to_string(),
            "s2|completed".to_string(),
            "s3|completed".to_string(),
        ],
        "the folded read-model rows"
    );
    // The workflow root's messages are the process lifecycle: one start and
    // one completion, and the root also announces itself as a task.
    assert!(kernel_started && kernel_completed);
    assert!(kernel_workflow, "the root announces itself as a task");
}

/// A container step's own message. A `uses` step announces its act's
/// completion when the act finishes — the hop that fans its children out —
/// while a package-less one announces the task when its children settled and
/// the join completed it. A branch task announces nothing at all.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_container_messages_are_emitted() {
    /// The folded (node, state, uses) message set of a workflow whose
    /// containers are a package-less step and a `uses` step, both with a
    /// branch child.
    async fn run() -> Vec<String> {
        let mut config = crate::Config::default();
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let folded: Arc<parking_lot::Mutex<Vec<String>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        {
            let folded = folded.clone();
            engine.channel().on_message(move |e| {
                let folded = folded.clone();
                async move {
                    if e.r#type == "workflow" {
                        return;
                    }
                    let node = if e.r#type == "act" {
                        e.inputs
                            .get::<crate::Vars>(crate::utils::consts::STEP_KEY)
                            .and_then(|step| step.get::<String>(crate::utils::consts::STEP_NODE_ID))
                            .unwrap_or_else(|| e.nid.clone())
                    } else {
                        e.nid.clone()
                    };
                    folded
                        .lock()
                        .push(format!("{node}|{}|{}", e.state, e.uses.is_some()));
                }
            });
        }
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let workflow = crate::model::Workflow::new()
            .with_id("wf-container-messages")
            .with_step(|step| {
                step.with_id("s1").with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("true")
                        .with_step(|step| step.with_id("s11"))
                })
            })
            .with_step(|step| {
                step.with_id("s2")
                    .with_uses(
                        crate::utils::test::USES_SET,
                        crate::Vars::new().with("k1", "v1"),
                    )
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_if("true")
                            .with_step(|step| step.with_id("s21"))
                    })
            });
        let runtime = engine.runtime();
        runtime.start(&workflow, crate::Vars::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("the run never finished");
        settle_outbox(runtime.kernel()).await;
        let mut folded = folded.lock().clone();
        folded.sort();
        folded.dedup();
        engine.close().await;
        folded
    }

    let kernel = run().await;
    // The package-less container's own completion, and the act's: both
    // steps speak.
    assert!(
        kernel.iter().any(|entry| entry == "s1|completed|false"),
        "a package-less container announces its own completion: {kernel:?}"
    );
    assert!(
        kernel
            .iter()
            .any(|entry| entry.starts_with("s2|completed") && entry.ends_with("true")),
        "a `uses` container announces its act's completion: {kernel:?}"
    );
    // A branch task is silent.
    assert!(
        !kernel
            .iter()
            .any(|entry| entry.starts_with("b1|") || entry.starts_with("b2|")),
        "branch tasks announce nothing: {kernel:?}"
    );
}

/// The published example model (`examples/catches/model.yml`) runs as it is:
/// its catch handler, its branches and the handler's inner steps are all
/// unnamed, so the import assigns the ids and the run can address them. A
/// *successful* act completion leaves the handlers undispatched, and the run
/// announces the two act keys in key order.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn the_unnamed_catch_example_runs() {
    const PID: &str = "p_unnamed_catch";

    /// Complete the init act, then the working act without an error, and
    /// hand back the act keys the run announced.
    async fn run() -> Vec<String> {
        // The published example, loaded as it is: its handler, branches and
        // inner steps are unnamed.
        let workflow = crate::model::Workflow::from_yml(include_str!(
            "../../../../../../examples/catches/model.yml"
        ))
        .unwrap();
        let mut config = crate::Config::default();
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let keys = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let wait_for_created = |key: &'static str| {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            engine.channel().on_message(move |e| {
                let done = done_rx.clone();
                async move {
                    if e.is_params_key(key) && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let init_created = wait_for_created("init");
        let act1_created = wait_for_created("act1");
        {
            let keys = keys.clone();
            engine.channel().on_message(move |e| {
                let keys = keys.clone();
                async move {
                    if let Some(params) = e.inputs.get::<crate::Vars>("params")
                        && let Some(key) = params.get::<String>("key")
                    {
                        let mut keys = keys.lock();
                        if !keys.iter().any(|seen| seen == &key) {
                            keys.push(key);
                        }
                    }
                }
            });
        }
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let runtime = engine.runtime();
        runtime
            .start(
                &workflow,
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, PID),
            )
            .await
            .unwrap();
        let (pid, tid) =
            tokio::time::timeout(std::time::Duration::from_secs(30), init_created.recv())
                .await
                .expect("the init act never appeared")
                .clone();
        runtime
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the init act must complete");
        let (pid, tid) =
            tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
                .await
                .expect("the working act never appeared")
                .clone();
        runtime
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the working act must complete");
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("the run never finished");
        runtime.kernel().dispatch_outbox().await.unwrap();
        let mut keys = keys.lock().clone();
        keys.sort();
        engine.close().await;
        keys
    }

    let kernel = run().await;
    assert!(
        !kernel
            .iter()
            .any(|key| key == "others" || key == "catch1" || key == "catch2"),
        "a successful completion never dispatches the handlers: {kernel:?}"
    );
    assert_eq!(
        kernel,
        vec!["act1".to_string(), "init".to_string()],
        "the example's two acts, in key order"
    );
}

/// A step declaring both `catches` and `timeouts`. The tick fires a timeout
/// branch while the step waits on its act; the act then fails; the handler
/// answers the error — and the step still waits for the timeout branch it
/// fired (`auto_complete` gates on every child of the step), so the
/// *decision* comes only when that branch settles too, triggered by the
/// branch's own settlement rather than the handler's.
///
/// Kernel only: the legacy engine wedges on this failure path (its failed
/// act never finishes propagating), which the catch matrix records.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_failed_step_waits_for_its_timeout_branch_before_the_catch_join() {
    const PID: &str = "p_mixed_catch_timeout";
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let wait_for_created = |key: &'static str| {
        let sig = engine.signal((String::new(), String::new()));
        let (done, done_rx) = (sig.clone(), sig.clone());
        engine.channel().on_message(move |e| {
            let done = done_rx.clone();
            async move {
                if e.is_params_key(key) && e.is_state(crate::MessageState::Created) {
                    done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                    done.close();
                }
            }
        });
        done
    };
    let act1_created = wait_for_created("act1");
    let branch_created = wait_for_created("branch");
    let handler_created = wait_for_created("handler");
    let workflow = crate::model::Workflow::new()
        .with_id("wf-mixed-catch-timeout")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act1"),
                )
                .with_catch(|catch| {
                    catch.with_id("c1").with_if("$ecode() == 'E1'").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "handler"),
                    )
                })
                .with_timeout(|timeout| {
                    timeout.with_id("t1").with_if("$cost() >= 300").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "branch"),
                    )
                })
        });
    let runtime = engine.runtime();
    runtime
        .start(
            &workflow,
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, PID),
        )
        .await
        .unwrap();
    let host = runtime.kernel().clone();
    let (pid, act1) = tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
        .await
        .expect("the working act never appeared")
        .clone();
    // The tick fires the timeout branch while the act still waits.
    let (_, branch) =
        tokio::time::timeout(std::time::Duration::from_secs(30), branch_created.recv())
            .await
            .expect("the timeout branch never appeared")
            .clone();
    // The act fails: the handler is dispatched, the step keeps running.
    runtime
        .do_action(&crate::event::Action::new(
            &pid,
            &act1,
            crate::event::EventAction::Error,
            crate::Vars::new()
                .with(crate::utils::consts::ACT_ERR_CODE, "E1")
                .with(crate::utils::consts::ACT_ERR_MESSAGE, "boom"),
        ))
        .await
        .expect("the act must error");
    let (_, handler) =
        tokio::time::timeout(std::time::Duration::from_secs(30), handler_created.recv())
            .await
            .expect("the handler never appeared")
            .clone();

    // The handler settles, but the fired timeout branch has not: the step
    // waits for it (the gate covers every child).
    runtime
        .do_action(&crate::event::Action::new(
            &pid,
            &handler,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the handler must complete");
    let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");
    assert_eq!(
        state_of(&snapshot, "c1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "t1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the timeout branch is still in flight"
    );
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "a settled handler does not decide the step while its branch runs"
    );

    // The branch settles: its settlement triggers the catch join, which
    // catches the error and completes the step.
    runtime
        .do_action(&crate::event::Action::new(
            &pid,
            &branch,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the timeout branch must complete");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !host.process_completed(PID).await.unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "the run never settled: {:?}",
            host.snapshot(PID)
                .await
                .unwrap()
                .expect("the aggregate")
                .tasks
                .iter()
                .map(|(id, task)| format!("{}:{:?}", id.node.as_str(), task.state))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the caught error completed the step"
    );
    engine.close().await;
}

/// A client `Cancel`'s acceptance surface. The kernel's action targets the
/// node the act folds into and refuses unless the step's own work is done
/// (`is_biz_success`) — its cancel redoes that step and undoes the chain
/// forward from it, which only makes sense once the step ran. The refusal
/// consumes nothing.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_cancel_refuses_a_step_whose_work_is_not_done() {
    const PID: &str = "p_cancel_guard";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-cancel-guard")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act1"),
                )
            })
            .with_step(|step| step.with_id("s2"))
    };
    let workflow = workflow();

    // The client's Cancel lands on the waiting act, whose step has not
    // finished its own work — refused, consuming nothing.
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let runtime = engine.runtime();
    runtime
        .start(
            &workflow,
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, PID),
        )
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if host_state(&runtime, PID, "s1").await
            == Some(crate::scheduler::core::model::task_graph::TaskState::Waiting)
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "s1 never waited");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let refused = runtime
        .do_action(&crate::event::Action::new(
            PID,
            &tid_of(PID, "s1", 0),
            crate::event::EventAction::Cancel,
            crate::Vars::new(),
        ))
        .await
        .expect_err("the kernel refuses a cancel whose step work is not done");
    assert!(
        format!("{refused:?}").contains("ActionNotAllowed"),
        "the kernel refuses on the same surface: {refused:?}"
    );
    // … and the refusal consumed nothing: the step is still waiting on its
    // act, one generation, nothing rewound.
    assert_eq!(
        host_state(&runtime, PID, "s1").await,
        Some(crate::scheduler::core::model::task_graph::TaskState::Waiting)
    );
    let host = runtime.kernel();
    let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");
    assert_eq!(tasks_of(&snapshot, "s1"), vec![0]);
    engine.close().await;
}

/// Drive one legacy-shaped cancel flow on one engine, the way a client
/// would (the `pack_irq_cancel_normal` shape): complete the head act,
/// cancel it once the chain's next act shows up, then answer the redone
/// acts until the run settles. Returns the engine and the cancel's outcome.
async fn run_cancel_engine(
    pid: &str,
    workflow: &crate::model::Workflow,
) -> (crate::Engine, std::result::Result<(), String>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    engine.channel().on_message(move |e| {
        let tx = tx.clone();
        async move {
            if !e.is_type("act") || !e.is_state(crate::MessageState::Created) {
                return;
            }
            let key = e
                .params()
                .and_then(|params| params.get::<String>("key"))
                .unwrap_or_default();
            let _ = tx.send((key, e.tid.clone()));
        }
    });
    let runtime = engine.runtime().clone();
    runtime
        .start(
            workflow,
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
        )
        .await
        .unwrap();
    let act1 = next_created_for(&mut rx, "act1").await;
    runtime
        .do_action(&crate::event::Action::new(
            pid,
            &act1,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the head act completes");
    next_created_for(&mut rx, "act2").await;
    // The cancelled act's own step has finished its work and the chain's
    // next act waits: the cancel both guards accept.
    let cancel = runtime
        .do_action(&crate::event::Action::new(
            pid,
            &act1,
            crate::event::EventAction::Cancel,
            crate::Vars::new(),
        ))
        .await
        .map_err(|error| error.to_string());
    if cancel.is_ok() {
        // The rewind redid the head step: answer it and its downstream.
        let redo = next_created_for(&mut rx, "act1").await;
        runtime
            .do_action(&crate::event::Action::new(
                pid,
                &redo,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the redone act completes");
        let again = next_created_for(&mut rx, "act2").await;
        runtime
            .do_action(&crate::event::Action::new(
                pid,
                &again,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the redone downstream completes");
    }
    wait_kernel_settled(runtime.kernel(), pid).await;
    (engine, cancel)
}

/// `Cancel` rewinds: a client cancels the head act once its step finished its
/// own work and the chain's next act waits, and the engine redoes that step
/// while undoing the chain forward of it.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_cancel_rewinds_a_finished_step() {
    const PID: &str = "p_cancel_parity";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-cancel-parity")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act1"),
                )
            })
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act2"),
                )
            })
    };
    let (kernel_engine, cancel) = run_cancel_engine(PID, &workflow()).await;
    assert!(
        cancel.is_ok(),
        "the guard accepts a cancel of a finished step: {cancel:?}"
    );

    let kernel_runtime = kernel_engine.runtime();
    let host = kernel_runtime.kernel();
    let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        tasks_of(&snapshot, "s1"),
        vec![0, 1],
        "the rewind redid the head step once"
    );
    kernel_engine.close().await;
}

/// The refusals the legacy guard makes, mirrored: a step whose own work is
/// not done, a step whose forward chain already settled (the
/// duplicate-delivery guard), and a step with no forward work at all.
#[tokio::test]
async fn a_cancel_needs_a_finished_step_with_live_forward_work() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let workflow = crate::model::Workflow::new()
        .with_id("wf-cancel-guard")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act1"),
            )
        })
        .with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act2"),
            )
        });
    let spec = ProcessSpec::from_workflow("p-cancel-guard", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    // The head step's own work is not done (its act waits): the legacy
    // `is_biz_success` refusal, consuming nothing.
    let before = host.snapshot("p-cancel-guard").await.unwrap().unwrap();
    let waiting = host
        .apply_action(
            "p-cancel-guard",
            &tid_of("p-cancel-guard", "s1", 0),
            ActionKind::Cancel,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert!(
        format!("{waiting}").contains("ActionNotAllowed"),
        "a waiting step is not cancelable: {waiting}"
    );
    let after = host.snapshot("p-cancel-guard").await.unwrap().unwrap();
    assert_eq!(
        before.version, after.version,
        "a refused cancel consumes nothing"
    );

    // Finished, but the chain forward of it settled: the legacy
    // duplicate-delivery guard ("not allowed to cancel").
    host.apply_action(
        "p-cancel-guard",
        &tid_of("p-cancel-guard", "s1", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(
        "p-cancel-guard",
        &tid_of("p-cancel-guard", "s2", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(host.process_completed("p-cancel-guard").await.unwrap());
    let settled = host
        .apply_action(
            "p-cancel-guard",
            &tid_of("p-cancel-guard", "s1", 0),
            ActionKind::Cancel,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert!(
        format!("{settled}").contains("not allowed to cancel"),
        "a settled chain refuses the rewind: {settled}"
    );

    // A lone step has no forward work to rewind at all: the legacy
    // "cannot find cancelled tasks".
    let lone = crate::model::Workflow::new()
        .with_id("wf-cancel-lone")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act1"),
            )
        });
    let spec = ProcessSpec::from_workflow("p-cancel-lone", &lone, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(
        "p-cancel-lone",
        &tid_of("p-cancel-lone", "s1", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    let empty = host
        .apply_action(
            "p-cancel-lone",
            &tid_of("p-cancel-lone", "s1", 0),
            ActionKind::Cancel,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert!(
        format!("{empty}").contains("cannot find cancelled tasks"),
        "a lone step has nothing to rewind: {empty}"
    );
}

/// Await the next act `Created` message for one params key (the cancel
/// script's only interesting event).
async fn next_created_for(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<(String, String)>,
    key: &str,
) -> String {
    loop {
        let (seen, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("the act '{key}' never appeared"))
            .expect("the channel is open");
        if seen == key {
            return tid;
        }
    }
}

/// One node's state in a running kernel driver, or `None` while the
/// aggregate is not readable yet.
async fn host_state(
    runtime: &Arc<crate::scheduler::Runtime>,
    pid: &str,
    node: &str,
) -> Option<crate::scheduler::core::model::task_graph::TaskState> {
    let snapshot = runtime.kernel().snapshot(pid).await.ok()??;
    snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node.as_str() == node)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, task)| task.state)
}

/// A client's `act:error` carries the legacy inputs (`ecode`, `message`):
/// the failure must land on the same node with the same code on both
/// engines — what `$ecode()` reads for a catch is this code.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_error_action_carries_the_legacy_inputs() {
    use crate::event::EventAction;

    /// Drive one engine: start, error the waiting act with `ecode`, then
    /// hand back the step's durable error.
    async fn run_error() -> Option<String> {
        let workflow = crate::model::Workflow::new()
            .with_id("wf-error-action")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "err-act"),
                )
            });
        let mut config = crate::Config::default();
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
        engine.channel().on_message(move |e| {
            let tx = tx.clone();
            async move {
                if e.r#type == "act"
                    && e.is_params_key("err-act")
                    && e.state == crate::MessageState::Created
                {
                    let _ = tx.send((e.pid.clone(), e.tid.clone()));
                }
            }
        });
        let runtime = engine.runtime();
        runtime
            .start(
                &workflow,
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, "p_err_action"),
            )
            .await
            .unwrap();
        let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .expect("the act was never created")
            .expect("the event channel closed");
        runtime
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                EventAction::Error,
                crate::Vars::new()
                    .with(crate::utils::consts::ACT_ERR_CODE, "boom")
                    .with(crate::utils::consts::ACT_ERR_MESSAGE, "boom-msg"),
            ))
            .await
            .expect("the act must error");
        // Wait for the walk to settle, then read the step's durable error.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            runtime.flush_writer().await.unwrap();
            let rows = runtime
                .store()
                .tasks()
                .query_all(&crate::store::query::Query::new().filter(
                    crate::store::query::Filter::and().expr(crate::store::query::Expr::eq(
                        "pid",
                        "p_err_action".to_string(),
                    )),
                ))
                .await
                .unwrap();
            let step_error = rows.iter().find_map(|row| {
                let node: crate::scheduler::core::model::view::NodeData =
                    serde_json::from_str(&row.node_data).ok()?;
                (node.id == "s1").then(|| row.err.clone()).flatten()
            });
            if let Some(error) = step_error {
                engine.close().await;
                return Some(error);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the step never carried an error: {rows:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    let kernel = run_error().await.expect("the step error");
    assert!(
        kernel.contains("\"ecode\":\"boom\"") && kernel.contains("boom-msg"),
        "the step error carries the client's ecode and message: {kernel}"
    );
}

/// The read model across a Back/redo: the kernel projects the *newest*
/// generation of every node (one row per node), while the legacy rows keep
/// the redo's history — the backed instance next to the redone one. Both
/// agree on the surviving instance's state.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_back_read_model_serves_the_redone_generation() {
    use crate::event::EventAction;

    /// Drive one engine through: start, Back to `s1` when the act is
    /// created, wait for the redone act, then hand back the process's rows.
    async fn run_back() -> Vec<crate::store::data::Task> {
        const PID: &str = "p_back_rows";
        let workflow = crate::model::Workflow::new()
            .with_id("wf-back-rows")
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "back-act"),
                )
            });
        let mut config = crate::Config::default();
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
        engine.channel().on_message(move |e| {
            let tx = tx.clone();
            async move {
                if e.r#type == "act"
                    && e.is_params_key("back-act")
                    && e.state == crate::MessageState::Created
                {
                    let _ = tx.send((e.pid.clone(), e.tid.clone()));
                }
            }
        });
        let runtime = engine.runtime();
        runtime
            .start(
                &workflow,
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, PID),
            )
            .await
            .unwrap();
        let (pid_now, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .expect("the act was never created")
            .expect("the event channel closed");
        runtime
            .do_action(&crate::event::Action::new(
                &pid_now,
                &tid,
                EventAction::Back,
                crate::Vars::new().with(crate::utils::consts::ACT_TO, "s1"),
            ))
            .await
            .expect("the act must back");
        // The redo re-runs s1 and parks a *new* s2 instance: its Created
        // message is the point where both engines' rows are settled.
        tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .expect("the redone act was never created");
        runtime.flush_writer().await.unwrap();
        let rows = runtime
            .store()
            .tasks()
            .query_all(
                &crate::store::query::Query::new().filter(
                    crate::store::query::Filter::and()
                        .expr(crate::store::query::Expr::eq("pid", PID.to_string())),
                ),
            )
            .await
            .unwrap();
        engine.close().await;
        rows
    }

    /// The states the rows name for one *node*: an act row folds to its
    /// owning step, the workflow root is `$` (the legacy `tid`s are random
    /// — only `node_data` names the model node).
    fn states_of(rows: &[crate::store::data::Task], node: &str) -> Vec<String> {
        let node_id = |tid: &str| {
            rows.iter()
                .find(|row| row.tid == tid)
                .and_then(|row| {
                    serde_json::from_str::<crate::scheduler::core::model::view::NodeData>(
                        &row.node_data,
                    )
                    .ok()
                })
                .map(|data| data.id)
        };
        let mut states: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                let row_node = if row.kind == "workflow" {
                    crate::utils::consts::TASK_ROOT_TID.to_string()
                } else if row.kind == "act" {
                    row.parent
                        .as_deref()
                        .and_then(node_id)
                        .unwrap_or_else(|| row.tid.clone())
                } else {
                    node_id(&row.tid).unwrap_or_else(|| row.tid.clone())
                };
                (row_node == node).then(|| row.state.clone())
            })
            .collect();
        states.sort();
        states.dedup();
        states
    }

    let kernel = run_back().await;
    // C5: the rows are the *instances*, history included — the redo backs s2
    // gen 0 and re-runs it (gen 1 waits again), s1 runs to completion in both
    // its generations, so a node's states are its generations' states.
    assert_eq!(
        states_of(&kernel, "s2"),
        vec!["backed".to_string(), "interrupted".to_string()],
        "the rows are the instances, history included"
    );
    assert_eq!(states_of(&kernel, "s1"), vec!["completed".to_string()]);
    // The root diverges and is registered in the migration notes: the
    // kernel completes the workflow root at its own hop, so a redo leaves
    // it (and the projected process row) `completed` while the run is still
    // in flight, where the legacy root waits for every child instance.
    // C5: a row is one *instance* — the redo's generations are all here
    // (s1 gen 0 backed + gen 1 completed, s2 gen 0 interrupted + gen 1
    // waiting), the root ran once. The pid rides the rows.
    let pid = kernel[0].pid.clone();
    assert_eq!(
        kernel
            .iter()
            .filter(|row| row.tid == tid_of(&pid, "$", 0))
            .count(),
        1,
        "one row per instance: the root"
    );
    assert_eq!(
        kernel
            .iter()
            .filter(|row| row.tid == tid_of(&pid, "s1", 0) || row.tid == tid_of(&pid, "s1", 1))
            .count(),
        2,
        "one row per instance: both s1 generations"
    );
    assert_eq!(
        kernel
            .iter()
            .filter(|row| row.tid == tid_of(&pid, "s2", 0) || row.tid == tid_of(&pid, "s2", 1))
            .count(),
        2,
        "one row per instance: both s2 generations"
    );
}

/// A branch predicate that calls an engine function (`$env.WORK_DIR`, read
/// from the process env) must decide the branches — the built-in subset
/// evaluator alone cannot answer it.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_env_function_predicate_decides_branches() {
    let root = std::env::temp_dir().join(format!("acts-kernel-pred-{}", crate::utils::longid()));
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-env-pred")
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b_true")
                            .with_if("$env.WORK_DIR != 'x'")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b_false")
                            .with_if("$env.WORK_DIR == 'x'")
                            .with_step(|step| step.with_id("s21"))
                    })
            })
    };
    let pid = "p_env_pred";
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    config.data.workdir = Some(root.display().to_string());

    // The predicate runs on a headless task of the engine.
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let settled = engine.signal(());
    let done = settled.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    let runtime = engine.runtime();
    runtime
        .start(
            &workflow(),
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), settled.recv())
        .await
        .expect("the kernel run never settled");
    let host = runtime.kernel().clone();
    let snapshot = host.snapshot(pid).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "b_true"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "b_false"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert!(!has_task(&snapshot, "s21"));

    std::fs::remove_dir_all(&root).ok();
    engine.close().await;
}

/// A branch predicate that cannot be evaluated fails its branch (the
/// legacy branch task's `init` error), the container and its ancestors
/// walk up `Failed` and the downstream chain skips — the legacy fold must
/// agree per node.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_invalid_branch_predicate_fails_its_branch() {
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-invalid-if")
            .with_step(|step| step.with_id("s0"))
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("missing_name > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            })
            .with_step(|step| step.with_id("s2"))
    };
    let pid = "p_invalid_if";

    // The failing branch fails the container and walks the ancestors up.
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let settled = engine.signal(());
    for handle in [settled.clone(), settled.clone()] {
        engine.channel().on_error(move |_| {
            let handle = handle.clone();
            async move { handle.close() }
        });
    }
    let done = settled.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    let runtime = engine.runtime();
    runtime
        .start(
            &workflow(),
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), settled.recv())
        .await
        .expect("the kernel run never settled");
    let host = runtime.kernel().clone();
    let snapshot = host.snapshot(pid).await.unwrap().unwrap();
    // The failing branch carries the predicate error as its own failure.
    let b1 = crate::scheduler::core::model::task_graph::TaskId::new("b1", 0).unwrap();
    assert_eq!(
        snapshot.tasks.get(&b1).map(|task| task.state),
        Some(crate::scheduler::core::model::task_graph::TaskState::Failed)
    );
    assert!(
        snapshot
            .variables
            .contains_key(&(b1, "error.message".to_string()))
    );
    engine.close().await;
}

/// Drive the kernel through a `catches` workflow: complete the `init` act,
/// then either error the `act1` act with `ecode` or complete it (which is
/// what dispatches the step's `catches` without an error to answer). The
/// engine is handed back alive so the caller can wait for its nodes.
///
/// A caught act error resumes the step per the `auto_complete` semantics:
/// the failed step is decided only once its handlers settle. A successful
/// completion leaves the handlers undispatched.
async fn run_catch_engine(
    pid: &str,
    ecode: Option<&str>,
    workflow: &crate::model::Workflow,
) -> crate::Engine {
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let runtime = engine.runtime().clone();
    // Subscribe before the start: the act's Created message fires the
    // moment the act runs, and a late subscriber misses it.
    let wait_for_created = |key: &'static str| {
        let sig = engine.signal((String::new(), String::new()));
        let (done, done_rx) = (sig.clone(), sig.clone());
        engine.channel().on_message(move |e| {
            let done = done_rx.clone();
            let key = key.to_string();
            async move {
                if e.is_params_key(&key) && e.is_state(crate::MessageState::Created) {
                    done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                    done.close();
                }
            }
        });
        done
    };
    let init_created = wait_for_created("init");
    let act1_created = wait_for_created("act1");
    runtime
        .start(
            workflow,
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
        )
        .await
        .unwrap();
    let (pid_now, tid) =
        tokio::time::timeout(std::time::Duration::from_secs(30), init_created.recv())
            .await
            .expect("the init act never appeared")
            .clone();
    runtime
        .do_action(&crate::event::Action::new(
            &pid_now,
            &tid,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the init act must complete");
    let (pid_now, tid) =
        tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
            .await
            .expect("the act1 act never appeared")
            .clone();
    match ecode {
        Some(ecode) => runtime
            .do_action(&crate::event::Action::new(
                &pid_now,
                &tid,
                crate::event::EventAction::Error,
                crate::Vars::new()
                    .with(crate::utils::consts::ACT_ERR_CODE, ecode)
                    .with(crate::utils::consts::ACT_ERR_MESSAGE, "boom"),
            ))
            .await
            .expect("the act1 act must error"),
        None => runtime
            .do_action(&crate::event::Action::new(
                &pid_now,
                &tid,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the act1 act must complete"),
    };
    engine
}

/// Wait until the kernel aggregate settled (its own completeness
/// predicate, read from one snapshot so the returned state is the one the
/// predicate saw), reporting the states when it never does.
async fn wait_kernel_settled(host: &Arc<KernelHost>, pid: &str) -> Snapshot {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let snapshot = host.snapshot(pid).await.unwrap().expect("aggregate exists");
        if snapshot.tasks.values().all(|task| task.state.is_terminal()) {
            return snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the kernel run never settled: {:?}; operations: {:?}",
            snapshot
                .tasks
                .iter()
                .map(|(id, task)| format!("{}:{:?}", id.node.as_str(), task.state))
                .collect::<Vec<_>>(),
            snapshot
                .operations
                .iter()
                .map(|(id, operation)| format!(
                    "{}:{}:{:?}",
                    id.task.node.as_str(),
                    id.sequence,
                    operation.state
                ))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// The `$ecode()`-selected handler answers the error: the handler's branch
/// runs a msg step. Both engines must route the same way — the legacy one
/// runs the handler subtree and then wedges at the failed step, which the
/// kernel completes with the caught error.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_catch_runs_the_selected_handler_on_error() {
    const PID: &str = "p_catch_caught";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-catch-caught")
            .with_step(|step| {
                step.with_id("s0").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "init"),
                )
            })
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "act1"),
                    )
                    .with_catch(|catch| {
                        catch.with_id("c1").with_branch(|branch| {
                            branch
                                .with_id("b1")
                                .with_if("$ecode() == 'err1'")
                                .with_step(|step| {
                                    step.with_id("h1").with_uses(
                                        crate::utils::test::USES_MSG,
                                        crate::Vars::new().with("key", "caught"),
                                    )
                                })
                        })
                    })
            })
    };
    let kernel_engine = run_catch_engine(PID, Some("err1"), &workflow()).await;
    let host = kernel_engine.runtime().kernel().clone();
    let snapshot = wait_kernel_settled(&host, PID).await;
    assert_no_folded_operations(&snapshot);

    // The `$ecode()`-selected handler subtree ran its msg step; the
    // unselected branch does not exist in this workflow.
    for node in ["c1", "b1", "h1"] {
        assert_eq!(
            state_of(&snapshot, node),
            crate::scheduler::core::model::task_graph::TaskState::Completed,
            "handler node '{node}'"
        );
    }
    // The caught error completes the step and the run.
    for (node, state) in [
        (
            "s1",
            crate::scheduler::core::model::task_graph::TaskState::Completed,
        ),
        (
            crate::utils::consts::TASK_ROOT_TID,
            crate::scheduler::core::model::task_graph::TaskState::Completed,
        ),
    ] {
        assert_eq!(state_of(&snapshot, node), state, "node '{node}'");
    }
    kernel_engine.close().await;
}

/// A successful completion does **not** run into a step's `catches`: the
/// legacy tree builds them as `NodeOutputKind::Catch` children, which
/// `Node::children()` (Normal kind) does not return, so
/// `check_in_children` never visits them — only `Step::on_error`
/// dispatches handlers, on the failure path. Both engines therefore finish
/// the step and the run with no handler task at all, which is what the fold
/// comparison pins (it is compared end to end because no handler exists to
/// diverge on).
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_catch_skips_handlers_on_success() {
    const PID: &str = "p_catch_success";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-catch-success")
            .with_step(|step| {
                step.with_id("s0").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "init"),
                )
            })
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "act1"),
                    )
                    .with_catch(|catch| {
                        catch
                            .with_id("c1")
                            .with_var("declared", 7)
                            .with_branch(|branch| {
                                branch
                                    .with_id("b1")
                                    .with_if("$ecode() == 'err1'")
                                    .with_step(|step| {
                                        step.with_id("h1").with_uses(
                                            crate::utils::test::USES_MSG,
                                            crate::Vars::new().with("key", "caught"),
                                        )
                                    })
                            })
                            .with_branch(|branch| {
                                branch.with_id("b2").with_else(true).with_step(|step| {
                                    step.with_id("h2").with_uses(
                                        crate::utils::test::USES_MSG,
                                        crate::Vars::new().with("key", "others"),
                                    )
                                })
                            })
                    })
            })
    };
    let kernel_engine = run_catch_engine(PID, None, &workflow()).await;
    let host = kernel_engine.runtime().kernel().clone();
    let snapshot = wait_kernel_settled(&host, PID).await;
    // The step and the run complete; no handler ever existed.
    for (node, state) in [
        (
            "s1",
            crate::scheduler::core::model::task_graph::TaskState::Completed,
        ),
        (
            crate::utils::consts::TASK_ROOT_TID,
            crate::scheduler::core::model::task_graph::TaskState::Completed,
        ),
    ] {
        assert_eq!(state_of(&snapshot, node), state, "node '{node}'");
    }
    for node in ["c1", "b1", "b2", "h1", "h2"] {
        assert!(
            !has_task(&snapshot, node),
            "a successful completion never dispatches handlers ('{node}')"
        );
    }
    kernel_engine.close().await;
}

/// Every handler skipped: the kernel resumes the error after its handlers
/// settled — the failed step is decided `Failed`, the walk decides the
/// ancestors and the downstream chain skips. The legacy engine reaches the
/// skipped dispatch (the same guard) and then wedges at the step.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_catch_resumes_the_error_when_every_handler_skipped() {
    const PID: &str = "p_catch_resumed";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-catch-resumed")
            .with_step(|step| {
                step.with_id("s0").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "init"),
                )
            })
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "act1"),
                    )
                    .with_catch(|catch| {
                        catch.with_id("c1").with_if("$ecode() == 'err1'").with_uses(
                            crate::utils::test::USES_MSG,
                            crate::Vars::new().with("key", "caught"),
                        )
                    })
            })
            .with_step(|step| step.with_id("s2"))
    };
    let kernel_engine = run_catch_engine(PID, Some("err2"), &workflow()).await;
    let host = kernel_engine.runtime().kernel().clone();
    let snapshot = wait_kernel_settled(&host, PID).await;
    for (node, state) in [
        (
            "c1",
            crate::scheduler::core::model::task_graph::TaskState::Skipped,
        ),
        (
            "s2",
            crate::scheduler::core::model::task_graph::TaskState::Skipped,
        ),
        (
            "s1",
            crate::scheduler::core::model::task_graph::TaskState::Failed,
        ),
        (
            crate::utils::consts::TASK_ROOT_TID,
            crate::scheduler::core::model::task_graph::TaskState::Failed,
        ),
    ] {
        assert_eq!(state_of(&snapshot, node), state, "node '{node}'");
    }
    assert!(
        !has_task(&snapshot, "h1"),
        "a skipped handler never dispatches its act"
    );
    kernel_engine.close().await;
}

#[tokio::test]
async fn manual_task_waits_for_the_client_action() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let mut spec = ProcessSpec::linear("p4", "root", &["wait", "after"]);
    // `wait` has no entry in the auto set: it is a waiting task.
    spec.auto.remove("wait");
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

    // `wait` ran and is Running; `after` must not have started.
    let snapshot = host.snapshot("p4").await.unwrap().unwrap();
    let state_of = |task: &str| {
        snapshot
            .tasks
            .iter()
            .find(|(id, _)| id.node.as_str() == task)
            .map(|(_, e)| e.state)
            .unwrap()
    };
    assert_eq!(
        state_of("wait"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    assert_eq!(
        state_of("after"),
        crate::scheduler::core::model::task_graph::TaskState::Created
    );

    // The client completes the waiting task; the flow moves on.
    host.apply_action(
        "p4",
        &tid_of("p4", "wait", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert!(host.process_completed("p4").await.unwrap());
}

/// A step declaring `timeouts` arms a durable timer at its start hop: while
/// the execution is live, a tick dispatches the first branch that has
/// neither fired (the one-shot marker) nor is in flight — one per round, in
/// declaration order, its window evaluated on the engine's environment (the
/// built-in evaluator decides the plain guards here). The owner may not
/// complete before the branch it fired settles, and the timer retires once
/// nothing can fire instead of waking forever.
#[tokio::test(flavor = "multi_thread")]
async fn the_timeout_timer_fires_a_branch_once_and_the_owner_waits_for_it() {
    const PID: &str = "p_timeout";
    let (host, mut wake_rx) = KernelHost::in_memory();
    // An unknown package: every `uses` step waits for a client action,
    // without a runner this host has none of.
    let workflow = crate::model::Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.test.wait", crate::Vars::new())
                .with_timeout(|timeout| {
                    timeout
                        .with_id("t1")
                        .with_if("true")
                        .with_uses("acts.test.wait", crate::Vars::new())
                })
                .with_timeout(|timeout| timeout.with_id("t2").with_if("false"))
        })
        .with_step(|step| step.with_id("s2"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

    let marker = |snapshot: &Snapshot, node: &str| -> Vec<String> {
        snapshot
            .variables
            .get(&(
                crate::scheduler::core::model::task_graph::TaskId::new(node, 0).unwrap(),
                crate::utils::consts::TASK_TIMEOUTS.to_string(),
            ))
            .map(|raw| serde_json::from_str(raw).unwrap())
            .unwrap_or_default()
    };

    // `s1` waits on the client — the live state a timed-out step is in.
    // The timer is armed by the start hop, which is not a wake source for
    // it: the legacy engine had no round before its first process tick, so
    // nothing fires until the scan wakes the timer on a tick.
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    assert!(
        !has_task(&snapshot, "t1") && !has_task(&snapshot, "t2"),
        "no branch fires before the first tick"
    );
    assert_eq!(marker(&snapshot, "s1"), Vec::<String>::new());

    // One tick: the open window dispatches, the closed one stays armed.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    host.tick(SystemClock.now_ms() + host.timer_poll_ms())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "t1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the branch ran into its own client wait"
    );
    assert_eq!(
        marker(&snapshot, "s1"),
        vec!["t1".to_string()],
        "the branch is claimed one-shot"
    );
    assert!(
        !has_task(&snapshot, "t2"),
        "a window that is not open dispatches nothing"
    );
    // The legacy task data the tick freezes: the owner's cost start and
    // the frozen cost, and the branch reads the cost through its scope
    // chain exactly like a legacy timeout child read its parent's.
    let owner = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    assert!(
        snapshot.variables.contains_key(&(
            owner.clone(),
            crate::scheduler::core::plan::timeout::STARTED_VAR.to_string()
        )),
        "the start hop stamps the cost start"
    );
    assert!(
        snapshot
            .variables
            .contains_key(&(owner, crate::utils::consts::TASK_COST.to_string())),
        "the tick freezes the cost (the legacy __cost datum)"
    );
    assert!(
        scope_vars(
            &snapshot,
            &crate::scheduler::core::model::task_graph::TaskId::new("t1", 0).unwrap()
        )
        .iter()
        .any(|(name, _)| name == crate::utils::consts::TASK_COST),
        "a timeout branch reads the frozen cost"
    );

    // A later tick re-evaluates (the timer stays armed) without ever
    // dispatching a branch that already fired.
    host.tick(SystemClock.now_ms() + host.timer_poll_ms())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        tasks_of(&snapshot, "t1").len(),
        1,
        "a one-shot branch fires once"
    );
    assert_eq!(marker(&snapshot, "s1"), vec!["t1".to_string()]);

    // The owner's act completing while its branch runs is only the act:
    // the step waits for the branch and the chain successor keeps waiting.
    host.apply_action(PID, &tid_of(PID, "s1", 0), ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the step stays running while the branch it fired settles (the legacy step's state)"
    );
    assert_eq!(
        state_of(&snapshot, "s2"),
        crate::scheduler::core::model::task_graph::TaskState::Created
    );

    // The branch settles: the join completes the owner, the flow moves on,
    // and the next tick retires the timer.
    host.apply_action(PID, &tid_of(PID, "t1", 0), ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    host.tick(SystemClock.now_ms() + host.timer_poll_ms())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert!(host.process_completed(PID).await.unwrap());
    assert!(
        snapshot
            .operations
            .values()
            .filter(|operation| operation.kind == OperationKind::DispatchTimeout)
            .all(|operation| matches!(operation.state, OperationState::Completed(_))),
        "a decided execution retires its timer"
    );
}

/// The workflow root is a container over the chain it seeded: it stays
/// `Running` until every seeded step settled (the legacy workflow task that
/// `auto_complete` finishes once its children are done), instead of
/// completing at its own hop while the chain still runs. That is the state
/// the read model projects for the run.
#[tokio::test(flavor = "multi_thread")]
async fn the_root_stays_running_until_its_seeded_chain_settled() {
    const PID: &str = "p_root_chain";
    let (host, mut wake_rx) = KernelHost::in_memory();
    // An unknown package: the first step waits for the client, so the chain
    // cannot finish.
    let workflow = crate::model::Workflow::new()
        .with_step(|step| {
            step.with_id("a")
                .with_uses("acts.test.wait", crate::Vars::new())
        })
        .with_step(|step| step.with_id("b"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the root waits for its chain"
    );
    assert_eq!(
        state_of(&snapshot, "a"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    assert_eq!(
        state_of(&snapshot, "b"),
        crate::scheduler::core::model::task_graph::TaskState::Created
    );
    assert!(!host.process_completed(PID).await.unwrap());

    // The chain head's act completes: the step and then the root finish.
    host.apply_action(PID, &tid_of(PID, "a", 0), ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "b"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the root joins once its seeded chain settled"
    );
    assert!(host.process_completed(PID).await.unwrap());
}

/// The two durable facts are different axes and must not clobber each
/// other: a catch handler is often itself a container, so its own fan-out
/// records the `children` flow — while the *role* its parent's catch join
/// reads back from it (`catch`) stays untouched. The whole mechanism runs
/// on it: the handler's branch settles, the handler completes, and the
/// catch join then decides the failed step.
#[tokio::test(flavor = "multi_thread")]
async fn a_handler_container_keeps_its_role_while_parked_for_children() {
    const PID: &str = "p_handler_container";
    let (host, mut wake_rx) = KernelHost::in_memory();
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("s1")
            .with_uses("acts.test.wait", crate::Vars::new())
            .with_catch(|catch| {
                catch.with_id("c1").with_if("true").with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("true")
                        .with_step(|step| step.with_id("s11"))
                })
            })
    });
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    // The client fails the waiting act: the step keeps running and
    // dispatches its handler, which fans its own branch out.
    host.apply_action(
        PID,
        &tid_of(PID, "s1", 0),
        ActionKind::Fail {
            code: "E1".into(),
            message: "boom".into(),
        },
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let c1 = crate::scheduler::core::model::task_graph::TaskId::new("c1", 0).unwrap();
    assert_eq!(
        crate::scheduler::core::plan::container::task_sign(&snapshot, &c1),
        Some(crate::scheduler::core::plan::container::SIGN_CATCH),
        "the handler keeps the role its parent's join reads"
    );
    assert_eq!(
        crate::scheduler::core::plan::container::task_flow(&snapshot, &c1),
        Some(crate::scheduler::core::plan::container::TaskFlow::Children),
        "the handler's own fan-out recorded the children flow"
    );
    // Its branch settled and the plain join completed the handler; the
    // catch join then caught the error on the step.
    assert_eq!(
        state_of(&snapshot, "c1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the catch join decided the failed step"
    );
    assert!(host.process_completed(PID).await.unwrap());
}

/// A step may declare both `branches` and `timeouts`: the tick fires a
/// branch while the step is still waiting on its act, and the act's
/// completion must still fan the step's declared branches out — a fired
/// timeout branch is not an "already fanned out" container child. The step
/// then waits for both child sets (the legacy `auto_complete` counts every
/// child task) and joins when the last one settles.
#[tokio::test(flavor = "multi_thread")]
async fn a_timeout_branch_does_not_mask_the_branches_of_the_same_step() {
    const PID: &str = "p_timeout_branch";
    let (host, mut wake_rx) = KernelHost::in_memory();
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("s1")
            .with_uses("acts.test.wait", crate::Vars::new())
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("true")
                    .with_step(|step| step.with_id("s11"))
            })
            .with_timeout(|timeout| {
                timeout
                    .with_id("t1")
                    .with_if("true")
                    .with_uses("acts.test.wait", crate::Vars::new())
            })
    });
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    // The timer evaluates from the first tick on: the start hop that armed
    // it does not wake it.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    host.tick(SystemClock.now_ms() + host.timer_poll_ms())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    assert_eq!(
        state_of(&snapshot, "t1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the timer fired its branch while the step waited on its act"
    );
    assert!(
        !has_task(&snapshot, "b1"),
        "the declared branches fan out on the act's completion"
    );

    // The act's completion: the step is not done (t1 is in flight) but its
    // branch child set comes alive.
    host.apply_action(PID, &tid_of(PID, "s1", 0), ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the step keeps running while the branches and the timeout it fired settle"
    );
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the selected branch ran (its own inner step is auto)"
    );

    // The last child settles: the join completes the step.
    host.apply_action(PID, &tid_of(PID, "t1", 0), ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert!(host.process_completed(PID).await.unwrap());
}

#[tokio::test]
async fn completing_a_terminal_task_is_rejected_without_consuming_state() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::linear("p5", "root", &[]);
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert!(host.process_completed("p5").await.unwrap());
    let err = host
        .apply_action(
            "p5",
            &tid_of("p5", "$", 0),
            ActionKind::Complete,
            Vec::new(),
        )
        .await
        .unwrap_err();
    let before = host.snapshot("p5").await.unwrap().unwrap();
    // The rejection text is the planner's; the contract is the refusal
    // plus a frozen aggregate.
    assert!(!format!("{err}").is_empty());
    let after = host.snapshot("p5").await.unwrap().unwrap();
    assert_eq!(
        before.version, after.version,
        "a rejected action must not move the aggregate"
    );
}

/// The skip cascade: cancelling the waiting head of a chain skips the
/// rest instead of erroring the propagation. Every execution ends
/// terminal, and the skipped task is identifiable by its state.
#[tokio::test]
async fn cancelled_head_cascades_skip_through_the_chain() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let mut spec = ProcessSpec::linear("p6", "root", &["head", "mid", "tail"]);
    spec.auto.remove("head"); // the head waits for a client action
    // … and so does the step its cancel rewinds: an auto downstream would
    // settle the moment the head did, leaving the rewind nothing to do
    // (the legacy guard refuses that).
    spec.auto.remove("mid");
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

    // The head's own work first: a linear spec has no acts, so the client
    // action is the work itself.
    host.apply_action(
        "p6",
        &tid_of("p6", "head", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    // The rewind: head gen0 keeps the legacy act's `Backed` fact, the
    // chain forward of it is skipped, and a fresh head generation is
    // already parked waiting for its own action.
    host.apply_action(
        "p6",
        &tid_of("p6", "head", 0),
        ActionKind::Cancel,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot("p6").await.unwrap().unwrap();
    let state_of = |task: &str, generation: u64| {
        snapshot
            .tasks
            .get(&crate::scheduler::core::model::task_graph::TaskId::new(task, generation).unwrap())
            .map(|task| task.state)
            .unwrap()
    };
    assert_eq!(
        state_of("head", 0),
        crate::scheduler::core::model::task_graph::TaskState::Backed
    );
    assert_eq!(
        state_of("mid", 0),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert_eq!(
        state_of("tail", 0),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert_eq!(
        state_of("head", 1),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );

    // The re-run answers its own action; the recreated downstream runs.
    host.apply_action(
        "p6",
        &tid_of("p6", "head", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(
        "p6",
        &tid_of("p6", "mid", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(host.process_completed("p6").await.unwrap());
    let snapshot = host.snapshot("p6").await.unwrap().unwrap();
    assert_eq!(
        snapshot
            .tasks
            .get(&crate::scheduler::core::model::task_graph::TaskId::new("head", 1).unwrap())
            .map(|task| task.state),
        Some(crate::scheduler::core::model::task_graph::TaskState::Completed)
    );
}

// ---- nested model import: the legacy branch-if suite, mirrored ----

type Snapshot = crate::scheduler::core::commit::aggregate::ProcessSnapshot;

fn state_of(
    snapshot: &Snapshot,
    task: &str,
) -> crate::scheduler::core::model::task_graph::TaskState {
    snapshot
        .tasks
        .iter()
        .filter(|(_, task_snapshot)| task_snapshot.model_node.as_str() == task)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, task_snapshot)| task_snapshot.state)
        .unwrap_or_else(|| panic!("execution for node '{task}' missing"))
}

fn has_task(snapshot: &Snapshot, task: &str) -> bool {
    snapshot
        .tasks
        .values()
        .any(|task_snapshot| task_snapshot.model_node.as_str() == task)
}

async fn run_branched(workflow: &crate::model::Workflow, pid: &str) -> Snapshot {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow(pid, workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    let report = host.run_until_quiescent(&mut wake_rx, 500).await.unwrap();
    assert_eq!(report.retried, 0, "every hop must land: {report:?}");
    let snapshot = host.snapshot(pid).await.unwrap().unwrap();
    // The fan-out's unselected branches, their cascade and the join are all
    // in the commits that decided them; nothing internal is left to drive.
    assert_no_folded_operations(&snapshot);
    snapshot
}

/// A branch whose `if` holds runs its subtree; the `else` sibling is one
/// Skipped execution and never creates its own steps (the legacy parity
/// floor: one terminal task per model node that legacy created).
#[tokio::test]
async fn branched_step_selects_the_true_branch_and_skips_the_else() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|step| step.with_id("s11"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_else(true)
                        .with_step(|step| step.with_id("s21"))
                })
        });
    let snapshot = run_branched(&workflow, "p-if-true").await;
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "b2"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert!(
        !has_task(&snapshot, "s21"),
        "unselected branch steps are not created"
    );
}

/// No `if` matches: the `else` branch runs.
#[tokio::test]
async fn else_branch_runs_when_no_condition_matches() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v < 0")
                        .with_step(|step| step.with_id("s11"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_else(true)
                        .with_step(|step| step.with_id("s21"))
                })
        });
    let snapshot = run_branched(&workflow, "p-else").await;
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert!(!has_task(&snapshot, "s11"));
    assert_eq!(
        state_of(&snapshot, "b2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s21"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
}

/// A branch with neither `if` nor `else` skips; the flow still finishes.
#[tokio::test]
async fn branch_without_if_or_else_skips_and_the_flow_finishes() {
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("step1").with_branch(|branch| {
            branch
                .with_id("b1")
                .with_step(|step| step.with_id("s11"))
                .with_step(|step| step.with_id("s12"))
        })
    });
    let snapshot = run_branched(&workflow, "p-empty-if").await;
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert!(!has_task(&snapshot, "s11") && !has_task(&snapshot, "s12"));
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
}

/// Every `true` branch runs — legacy evaluates each branch independently.
#[tokio::test]
async fn multiple_true_branches_all_run() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|step| step.with_id("s11"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_if("v >= 1")
                        .with_step(|step| step.with_id("s21"))
                })
        });
    let snapshot = run_branched(&workflow, "p-multi").await;
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "b2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert!(has_task(&snapshot, "s11") && has_task(&snapshot, "s21"));
}

/// The flow continues past the branched step to the next chain step, and
/// a branch's inner chain runs in order before the join completes.
#[tokio::test]
async fn branched_step_flows_to_the_next_chain_step() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("v > 0")
                    .with_step(|step| step.with_id("s11"))
                    .with_step(|step| step.with_id("s12"))
            })
        })
        .with_step(|step| step.with_id("step2"));
    let snapshot = run_branched(&workflow, "p-chain").await;
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "s12"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "step2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
}

/// Nesting composes: a branch containing a step that branches again.
#[tokio::test]
async fn nested_branched_step_inside_a_branch() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch.with_id("b1").with_if("v > 0").with_step(|step| {
                    step.with_id("mid")
                        .with_branch(|branch| {
                            branch
                                .with_id("c1")
                                .with_if("v > 0")
                                .with_step(|step| step.with_id("c1s"))
                        })
                        .with_branch(|branch| {
                            branch
                                .with_id("c2")
                                .with_else(true)
                                .with_step(|step| step.with_id("c2s"))
                        })
                })
            })
        });
    let snapshot = run_branched(&workflow, "p-nested").await;
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "mid"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "c1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "c2"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    assert!(!has_task(&snapshot, "c2s"));
}

/// A branch's waiting (IRQ) step parks the whole container chain; the
/// client completion resumes it through the joins.
#[tokio::test]
async fn waiting_inner_step_resumes_on_the_client_action() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch.with_id("b1").with_if("v > 0").with_step(|step| {
                    step.with_id("s11")
                        .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                })
            })
        });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-wait", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot("p-wait").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    // A container whose children run stays Running (the legacy step whose
    // `next` ran): b1 over s11, and step1 over b1.
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Running
    );
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Running
    );

    host.apply_action(
        "p-wait",
        &tid_of("p-wait", "s11", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(host.process_completed("p-wait").await.unwrap());
    let snapshot = host.snapshot("p-wait").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
}

/// A client completion of a waiting branched step fans out with the
/// action's inputs on the container's scope.
#[tokio::test]
async fn waiting_branched_step_fans_out_on_the_client_completion() {
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("w > 0")
                    .with_step(|step| step.with_id("s11"))
            })
    });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-manual", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(
        !has_task(&host.snapshot("p-manual").await.unwrap().unwrap(), "b1"),
        "no fan-out before the client decides"
    );

    host.apply_action(
        "p-manual",
        &tid_of("p-manual", "step1", 0),
        ActionKind::Complete,
        vec![("w".to_string(), "2".to_string())],
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(host.process_completed("p-manual").await.unwrap());
    let snapshot = host.snapshot("p-manual").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
}

/// Cancelling a step that finished its own work rewinds the chain (the
/// legacy guard): the closed generation keeps the legacy act's own `Backed`
/// fact, and the re-entered step fans out fresh generations of its branches
/// when it completes again.
#[tokio::test]
async fn cancelling_a_finished_container_rewinds_its_branches() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|step| step.with_id("s11"))
                })
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
        });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-cancel-early", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    // The step's own work first: its act, then the branches it dispatched.
    host.apply_action(
        "p-cancel-early",
        &tid_of("p-cancel-early", "step1", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    let snapshot = host.snapshot("p-cancel-early").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the container settled after its branches ran"
    );
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );

    host.apply_action(
        "p-cancel-early",
        &tid_of("p-cancel-early", "step1", 0),
        ActionKind::Cancel,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    let snapshot = host.snapshot("p-cancel-early").await.unwrap().unwrap();
    let state_at = |task: &str, generation: u64| {
        snapshot
            .tasks
            .get(&crate::scheduler::core::model::task_graph::TaskId::new(task, generation).unwrap())
            .map(|task| task.state)
            .unwrap()
    };
    assert_eq!(
        state_at("step1", 0),
        crate::scheduler::core::model::task_graph::TaskState::Backed,
        "the decided generation took the legacy cancelled act's fact"
    );
    assert_eq!(
        state_at("step1", 1),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the redone step waits on its own act"
    );

    // The re-entered step waits again; completing it fans out its branches
    // as fresh generations.
    host.apply_action(
        "p-cancel-early",
        &tid_of("p-cancel-early", "step1", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    host.apply_action(
        "p-cancel-early",
        &tid_of("p-cancel-early", "step2", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    assert!(host.process_completed("p-cancel-early").await.unwrap());

    let snapshot = host.snapshot("p-cancel-early").await.unwrap().unwrap();
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "step2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(tasks_of(&snapshot, "step1"), vec![0, 1]);
    assert_eq!(
        tasks_of(&snapshot, "b1"),
        vec![0, 1],
        "the re-entered container redrew its branches exactly once"
    );
}

/// Cancelling a container that finished its own work rewinds it: the
/// cascade closes the settled branch subtree, the rewind recreates it as
/// fresh generations, and the re-entered container's fan-out READMITS those
/// instead of allocating duplicates.
#[tokio::test]
async fn cancelling_a_finished_container_readmits_the_rewound_branches() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_branch(|branch| {
                    branch.with_id("b1").with_if("v > 0").with_step(|step| {
                        step.with_id("s11")
                            .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                    })
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_else(true)
                        .with_step(|step| step.with_id("s21"))
                })
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
        });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-cancel-mid", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    // The branches first: s11 waits on its act while the else branch is
    // skipped, so the container is parked `Running` (the legacy step's
    // state while its children run).
    let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Running
    );
    assert_eq!(
        state_of(&snapshot, "b2"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    host.apply_action(
        "p-cancel-mid",
        &tid_of("p-cancel-mid", "s11", 0),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the container settled once its branches did"
    );

    host.apply_action(
        "p-cancel-mid",
        &tid_of("p-cancel-mid", "step1", 0),
        ActionKind::Cancel,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();

    // The gen0 subtree is closed by the cascade, and the rewind has already
    // recreated it as Created generations. The root's own generation stays
    // live: it is the container over the whole chain, the rewind's fresh
    // generations included.
    let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
    assert!(
        host_all_terminal_generations_0(&snapshot),
        "gen0 subtree left open: {:?}",
        snapshot
            .tasks
            .iter()
            .filter(|(id, task)| {
                id.generation == 0
                    && id.node.as_str() != crate::utils::consts::TASK_ROOT_TID
                    && !task.state.is_terminal()
            })
            .map(|(id, task)| format!("{}:{:?}", id.node.as_str(), task.state))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the root outlives the generation its rewind closed"
    );
    // step1 is auto: the re-entered generation re-fans out on its own,
    // READMITTING the rewound branch generations (never a third one).
    assert_eq!(
        tasks_of(&snapshot, "b1"),
        vec![0, 1],
        "readmission must not allocate a third generation"
    );
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the readmitted branch container runs over its own inner step"
    );
    assert_eq!(
        state_of(&snapshot, "s11"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );

    host.apply_action(
        "p-cancel-mid",
        &tid_of("p-cancel-mid", "s11", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();
    host.apply_action(
        "p-cancel-mid",
        &tid_of("p-cancel-mid", "step2", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();
    assert!(host.process_completed("p-cancel-mid").await.unwrap());
    let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
    assert!(host_all_terminal(&snapshot));
    assert_eq!(
        state_of(&snapshot, "b1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_of(&snapshot, "step2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        tasks_of(&snapshot, "b1"),
        vec![0, 1],
        "the redone container redrew its branches exactly once"
    );
}

/// A branched workflow settles with the expected per-node terminal states:
/// the true branch's step runs, the `else` branch skips, and the chain
/// continues to the next step.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_path_settles_a_branched_workflow() {
    use crate::scheduler::core::model::task_graph::TaskState as S;

    let workflow = crate::model::Workflow::new()
        .with_id("wf-branch-parity")
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|step| step.with_id("s11"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_else(true)
                        .with_step(|step| step.with_id("s21"))
                })
        })
        .with_step(|step| step.with_id("step2"));

    let snapshot = run_branched(&workflow, "k-branch-parity").await;
    assert_eq!(state_of(&snapshot, "step1"), S::Completed);
    assert_eq!(state_of(&snapshot, "b1"), S::Completed);
    assert_eq!(state_of(&snapshot, "s11"), S::Completed);
    assert_eq!(state_of(&snapshot, "b2"), S::Skipped);
    assert!(
        !has_task(&snapshot, "s21"),
        "an unentered branch builds nothing"
    );
    assert_eq!(state_of(&snapshot, "step2"), S::Completed);
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        S::Completed
    );
    assert!(snapshot.tasks.values().all(|e| e.state.is_terminal()));
}

fn host_all_terminal(snapshot: &Snapshot) -> bool {
    snapshot.tasks.values().all(|e| e.state.is_terminal())
        && snapshot
            .operations
            .values()
            .all(|record| matches!(record.state, OperationState::Completed(_)))
}

/// Whether every generation-0 execution settled, the root excluded: the
/// root is the container over the whole seeded chain, so its own generation
/// stays live until every generation the rewinds created settled too.
fn host_all_terminal_generations_0(snapshot: &Snapshot) -> bool {
    snapshot
        .tasks
        .iter()
        .filter(|(id, _)| {
            id.generation == 0 && id.node.as_str() != crate::utils::consts::TASK_ROOT_TID
        })
        .all(|(_, task)| task.state.is_terminal())
}

/// A client action's intent is durable *as the action*: when the deciding
/// call never applies it (a crash, an expired lease), the recovery-driven
/// hop replays the client's decision — not the auto completion a bare
/// `ApplyAction` hop would plan.
#[tokio::test]
async fn a_recovered_action_intent_applies_the_action_not_a_completion() {
    const PID: &str = "p-recovered-action";
    let workflow = crate::model::Workflow::new()
        .with_id("wf-recovered-action")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act1"),
            )
        })
        .with_step(|step| step.with_id("s2"));
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    // The client's intent, committed exactly as the deciding call does it —
    // and then that call never runs (the hop below is all that is left).
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let task = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    let operation = OperationId {
        process_id: PID.into(),
        task: crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap(),
        sequence: snapshot
            .operations
            .keys()
            .filter(|id| id.task.node.as_str() == "s1" && id.task.generation == 0)
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            + 1,
    };
    let intent = KernelHost::action_intent(
        PID,
        &task,
        &operation,
        &ActionKind::Fail {
            code: "E9".into(),
            message: "recovered".into(),
        },
        &[],
        snapshot.version.clone(),
    );
    host.processes
        .commit_transition(&intent, SystemClock.now_ms(), host.timer_poll_ms())
        .await
        .unwrap();
    assert_eq!(
        state_of(&host.snapshot(PID).await.unwrap().unwrap(), "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the intent alone decides nothing"
    );

    host.tick(SystemClock.now_ms()).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Failed,
        "the hop applied the client's failure, not a completion"
    );
    assert_eq!(
        snapshot
            .variables
            .get(&(task, "error.code".to_string()))
            .map(String::as_str),
        Some("E9"),
        "the failure carries the client's code"
    );
    assert_eq!(
        state_of(&snapshot, "s2"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped,
        "the downstream is skipped by the error walk"
    );

    // The fallback stays: an `ApplyAction` operation with no action fact
    // (the inert carrier a rewind leaves behind) is the bare hop — the
    // auto completion.
    let (bare_host, mut bare_wake) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-bare-action", &workflow, &|_| None).unwrap();
    bare_host.start_process(&spec).await.unwrap();
    bare_host
        .run_until_quiescent(&mut bare_wake, 200)
        .await
        .unwrap();
    let snapshot = bare_host.snapshot("p-bare-action").await.unwrap().unwrap();
    let bare = TransitionPlan {
        process_id: "p-bare-action".into(),
        expected: snapshot.version.clone(),
        task_creates: Vec::new(),
        state_changes: Vec::new(),
        variable_writes: Vec::new(),
        source_operation: None,
        operations: vec![OperationIntent {
            id: OperationId {
                process_id: "p-bare-action".into(),
                task: crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap(),
                sequence: snapshot
                    .operations
                    .keys()
                    .filter(|id| id.task.node.as_str() == "s1" && id.task.generation == 0)
                    .map(|id| id.sequence)
                    .max()
                    .unwrap_or(0)
                    + 1,
            },
            kind: OperationKind::ApplyAction,
            ready_at_ms: None,
        }],
        outbox: Vec::new(),
    };
    bare_host
        .processes
        .commit_transition(&bare, SystemClock.now_ms(), bare_host.timer_poll_ms())
        .await
        .unwrap();
    bare_host.tick(SystemClock.now_ms()).await.unwrap();
    bare_host
        .run_until_quiescent(&mut bare_wake, 300)
        .await
        .unwrap();
    let snapshot = bare_host.snapshot("p-bare-action").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "a fact-less operation keeps the bare hop"
    );
    assert!(bare_host.process_completed("p-bare-action").await.unwrap());
}

fn tasks_of(snapshot: &Snapshot, task: &str) -> Vec<u64> {
    let mut generations: Vec<u64> = snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == task)
        .map(|id| id.generation)
        .collect();
    generations.sort_unstable();
    generations
}

/// A rewind is part of the decision that asks for it: the `Cancel`'s own
/// atomic commit closes the source generation, pins the target fact and — since
/// A3 folds the pure `ReenterTask` hand-off — creates the fresh generations and
/// their start operation in that same commit. Recovery alone no longer has to
/// drive a second hop; a crash before the commit loses the whole decision, and
/// a crash after it cannot lose the rewind.
#[tokio::test]
async fn a_cancel_carries_its_rewind_in_the_deciding_commit() {
    const PID: &str = "p-cancel-durable";
    let workflow = crate::model::Workflow::new()
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act1"),
            )
        })
        .with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "act2"),
            )
        });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    // The step's own work first: the legacy guard (which the kernel
    // mirrors) refuses a cancel before it.
    host.apply_action(PID, &tid_of(PID, "s1", 0), ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(PID, &tid_of(PID, "s1", 0), ActionKind::Cancel, Vec::new())
        .await
        .unwrap();
    // Durable before any hop ran: the decision closed the source, pinned the
    // rewind target and planned the re-entry inside its own commit.
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let source = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    assert_eq!(
        snapshot.tasks[&source].state,
        crate::scheduler::core::model::task_graph::TaskState::Backed,
        "the deciding commit closes the source generation"
    );
    assert_eq!(
        snapshot
            .variables
            .get(&(source.clone(), REENTER_TARGET_VAR.to_string()))
            .map(String::as_str),
        Some("s1"),
        "the deciding commit pins the target the rewind re-enters"
    );
    assert_eq!(
        tasks_of(&snapshot, "s1"),
        vec![0, 1],
        "the deciding commit creates the re-entered generation"
    );
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Ready,
        "the re-entered generation is ready for its start"
    );
    let rewind_start = snapshot
        .operations
        .iter()
        .find(|(id, _)| id.task.node.as_str() == "s1" && id.task.generation == 1)
        .map(|(_, operation)| operation.kind);
    assert_eq!(
        rewind_start,
        Some(OperationKind::ExecuteTask),
        "the folded re-entry hands the new generation to the ordinary start"
    );
    assert!(
        !snapshot.operations.values().any(|operation| matches!(
            operation.kind,
            OperationKind::ReenterTask
                | OperationKind::PropagateTask
                | OperationKind::SkipTask
                | OperationKind::AbortTask
        )),
        "no pure-internal operation may survive the deciding commit: {:?}",
        snapshot
            .operations
            .values()
            .map(|operation| operation.kind)
            .collect::<Vec<_>>()
    );

    // No wake survives from the deciding call: the rewind's start is driven by
    // the durable ready record, exactly like a run a crash left undecided.
    while wake_rx.try_recv().is_ok() {}
    host.tick(SystemClock.now_ms()).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        tasks_of(&snapshot, "s1"),
        vec![0, 1],
        "the rewind created one fresh generation"
    );
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the redone step waits on its own act again"
    );
    assert!(
        !snapshot.operations.values().any(|operation| matches!(
            operation.kind,
            OperationKind::ReenterTask
                | OperationKind::PropagateTask
                | OperationKind::SkipTask
                | OperationKind::AbortTask
        )),
        "the rewind's start and completion are the only durable operations"
    );
}

// ---- parity matrix: Abort / Back / Fail (Error) ----

/// Abort × IRQ: aborting a waiting step decides it `Aborted`, rewrites
/// its ancestor chain `Aborted` (the legacy `abort_task` walk — the root
/// included, terminal ancestors not spared), skips the untouched
/// downstream, and leaves the completed sibling alone.
#[tokio::test]
async fn abort_on_waiting_step_aborts_the_ancestor_chain() {
    let workflow = crate::model::Workflow::new()
        .with_step(|step| step.with_id("s1"))
        .with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "abort-act"),
            )
        })
        .with_step(|step| step.with_id("s3"));
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-abort", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    host.apply_action(
        "p-abort",
        &tid_of("p-abort", "s2", 0),
        ActionKind::Abort,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    let snapshot = host.snapshot("p-abort").await.unwrap().unwrap();
    assert!(host_all_terminal(&snapshot));
    let state = |task: &str| state_of(&snapshot, task);
    assert_eq!(
        state("s2"),
        crate::scheduler::core::model::task_graph::TaskState::Aborted
    );
    // The ancestor walk: the root — terminal since the seed — is
    // rewritten Aborted; the completed sibling is not an ancestor.
    assert_eq!(
        state("$"),
        crate::scheduler::core::model::task_graph::TaskState::Aborted
    );
    assert_eq!(
        state("s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    // The untouched downstream joins the death cascade.
    assert_eq!(
        state("s3"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
}

/// Abort × branches: aborting a waiting branch step ends the whole
/// container chain upward (branch, branched step, root) while the
/// already-settled sibling branch keeps its state.
#[tokio::test]
async fn abort_inside_a_branch_aborts_the_container_chain() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("step1")
                .with_branch(|branch| {
                    branch.with_id("b1").with_if("v > 0").with_step(|step| {
                        step.with_id("s11")
                            .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                    })
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_else(true)
                        .with_step(|step| step.with_id("s21"))
                })
        });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-abort-branch", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    host.apply_action(
        "p-abort-branch",
        &tid_of("p-abort-branch", "s11", 0),
        ActionKind::Abort,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();

    let snapshot = host.snapshot("p-abort-branch").await.unwrap().unwrap();
    assert!(host_all_terminal(&snapshot));
    let state = |task: &str| state_of(&snapshot, task);
    assert_eq!(
        state("s11"),
        crate::scheduler::core::model::task_graph::TaskState::Aborted
    );
    assert_eq!(
        state("b1"),
        crate::scheduler::core::model::task_graph::TaskState::Aborted
    );
    assert_eq!(
        state("step1"),
        crate::scheduler::core::model::task_graph::TaskState::Aborted
    );
    assert_eq!(
        state("$"),
        crate::scheduler::core::model::task_graph::TaskState::Aborted
    );
    assert_eq!(
        state("b2"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
}

/// Back × IRQ: backing the waiting step to its completed predecessor
/// leaves the old generation `Backed`, re-runs the named step as a fresh
/// generation, and recreates the downstream behind it.
#[tokio::test]
async fn back_redo_reruns_the_named_step_and_its_downstream() {
    let workflow = crate::model::Workflow::new()
        .with_step(|step| step.with_id("s1"))
        .with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "back-act"),
            )
        });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-back", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    host.apply_action(
        "p-back",
        &tid_of("p-back", "s2", 0),
        ActionKind::Back,
        vec![("to".to_string(), "s1".to_string())],
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    let snapshot = host.snapshot("p-back").await.unwrap().unwrap();
    let state_at = |task: &str, generation: u64| {
        snapshot
            .tasks
            .get(&crate::scheduler::core::model::task_graph::TaskId::new(task, generation).unwrap())
            .map(|task| task.state)
            .unwrap()
    };
    assert_eq!(
        state_at("s2", 0),
        crate::scheduler::core::model::task_graph::TaskState::Backed,
        "the backed generation keeps its decision"
    );
    assert_eq!(
        state_at("s1", 0),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    // The redo: s1 generation 1 already ran; s2 generation 1 waits again.
    assert_eq!(
        state_at("s1", 1),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state_at("s2", 1),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );

    host.apply_action(
        "p-back",
        &tid_of("p-back", "s2", 1),
        ActionKind::Complete,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    assert!(host.process_completed("p-back").await.unwrap());
}

/// Back without the client's `to` is refused before anything durable.
///
/// The refusal happens before the durable intent, exactly like the legacy
/// `EventAction::Back`, which reads `to` before it touches the task: the
/// state phase and the aggregate are untouched, and the client can retry
/// with the step it means.
#[tokio::test]
async fn back_without_to_is_refused_without_consuming_state() {
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("s1").with_uses(
            crate::utils::test::USES_IRQ,
            crate::Vars::new().with("key", "back-no-to"),
        )
    });
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-back-no-to", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    let before = host.snapshot("p-back-no-to").await.unwrap().unwrap();

    let error = host
        .apply_action(
            "p-back-no-to",
            &tid_of("p-back-no-to", "s1", 0),
            ActionKind::Back,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("'to'"));

    let after = host.snapshot("p-back-no-to").await.unwrap().unwrap();
    assert_eq!(
        before.version, after.version,
        "a refused back consumes nothing"
    );
    assert_eq!(
        state_of(&after, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the step keeps waiting on its act"
    );
}

/// Fail × IRQ: failing a waiting step decides it `Failed` (the legacy
/// `Error`), skips the untouched downstream, writes the error variables,
/// and ends the ancestor chain `Failed` (the legacy error walk).
#[tokio::test]
async fn fail_on_waiting_step_skips_downstream_and_decides_ancestors() {
    let workflow = crate::model::Workflow::new()
        .with_step(|step| step.with_id("s1"))
        .with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "fail-act"),
            )
        })
        .with_step(|step| step.with_id("s3"));
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("p-fail", &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    host.apply_action(
        "p-fail",
        &tid_of("p-fail", "s2", 0),
        ActionKind::Fail {
            code: "E42".into(),
            message: "boom".into(),
        },
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    let snapshot = host.snapshot("p-fail").await.unwrap().unwrap();
    assert!(host_all_terminal(&snapshot));
    let state = |task: &str| state_of(&snapshot, task);
    assert_eq!(
        state("s2"),
        crate::scheduler::core::model::task_graph::TaskState::Failed
    );
    assert_eq!(
        state("s3"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    // The completed sibling is not an ancestor; the root is.
    assert_eq!(
        state("s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state("$"),
        crate::scheduler::core::model::task_graph::TaskState::Failed
    );
    let s2 = crate::scheduler::core::model::task_graph::TaskId::new("s2", 0).unwrap();
    assert_eq!(
        snapshot
            .variables
            .get(&(s2, "error.code".to_string()))
            .map(String::as_str),
        Some("E42")
    );
}

/// Aborting a waiting act walks the ancestor chain up (`Aborted`) and skips
/// the chain downstream of the aborted step.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_abort_walks_up_and_skips_downstream() {
    use crate::scheduler::core::model::task_graph::TaskState as S;

    let workflow = || {
        crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "parity-abort"),
                )
            })
            .with_step(|step| step.with_id("s3"))
    };

    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("k-abort", &workflow(), &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(
        "k-abort",
        &tid_of("k-abort", "s2", 0),
        ActionKind::Abort,
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    assert_all_terminal(&host, "k-abort").await;
    let snapshot = host.snapshot("k-abort").await.unwrap().unwrap();
    assert_eq!(state_of(&snapshot, "s1"), S::Completed);
    assert_eq!(state_of(&snapshot, "s2"), S::Aborted);
    assert_eq!(state_of(&snapshot, "s3"), S::Skipped);
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        S::Aborted,
    );
}

/// Failing a waiting act decides the ancestors `Failed` and skips the chain
/// downstream of the failed step.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_fail_decides_ancestors_and_skips_downstream() {
    use crate::scheduler::core::model::task_graph::TaskState as S;

    let workflow = || {
        crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "parity-fail"),
                )
            })
            .with_step(|step| step.with_id("s3"))
    };

    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::from_workflow("k-fail", &workflow(), &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(
        "k-fail",
        &tid_of("k-fail", "s2", 0),
        ActionKind::Fail {
            code: "E42".into(),
            message: "boom".into(),
        },
        Vec::new(),
    )
    .await
    .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    assert_all_terminal(&host, "k-fail").await;
    let snapshot = host.snapshot("k-fail").await.unwrap().unwrap();
    assert_eq!(state_of(&snapshot, "s1"), S::Completed);
    assert_eq!(state_of(&snapshot, "s2"), S::Failed);
    assert_eq!(state_of(&snapshot, "s3"), S::Skipped);
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        S::Failed,
    );
}

/// A parent's own state while a child it fired is still in flight: the parked
/// parent is `Running`, the root included now that it is a container over its
/// seeded chain. This is exactly the state the read model projects
/// (`proc:get` reads `running` while children run), so it is read mid-flight
/// rather than after the run settled.
///
/// The branch's own state is deliberately not asserted here: its act is still
/// unanswered, so the node reads `Waiting` (the act folded into it).
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_parent_state_stays_running_while_a_fired_branch_runs() {
    const PID: &str = "p_timeout_parent";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-timeout-parent")
            .with_step(|step| {
                step.with_id("s0").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "init"),
                )
            })
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "act1"),
                    )
                    .with_timeout(|timeout| {
                        timeout.with_id("t1").with_if("$cost() >= 300").with_uses(
                            crate::utils::test::USES_IRQ,
                            crate::Vars::new().with("key", "branch"),
                        )
                    })
            })
    };
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);

    // The workflow runs on the kernel; the parent's own state is read from
    // the aggregate while its fired branch is still in flight.
    let kernel_engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let wait_for = |key: &'static str| {
        let sig = kernel_engine.signal((String::new(), String::new()));
        let (done, done_rx) = (sig.clone(), sig.clone());
        kernel_engine.channel().on_message(move |e| {
            let done = done_rx.clone();
            async move {
                if e.is_params_key(key) && e.is_state(crate::MessageState::Created) {
                    done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                    done.close();
                }
            }
        });
        done
    };
    let init_created = wait_for("init");
    let act1_created = wait_for("act1");
    let branch_created = wait_for("branch");
    let _kernel = kernel_engine
        .runtime()
        .start(
            &workflow(),
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, PID),
        )
        .await
        .unwrap();
    let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), init_created.recv())
        .await
        .expect("the init act never appeared")
        .clone();
    kernel_engine
        .runtime()
        .do_action(&crate::event::Action::new(
            &pid,
            &tid,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the init act must complete");
    let (pid, act1) = tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
        .await
        .expect("the act1 act never appeared")
        .clone();
    let (_, branch) =
        tokio::time::timeout(std::time::Duration::from_secs(30), branch_created.recv())
            .await
            .expect("the timeout branch never appeared")
            .clone();
    kernel_engine
        .runtime()
        .do_action(&crate::event::Action::new(
            &pid,
            &act1,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the act1 act must complete");
    let host = kernel_engine.runtime().kernel().clone();
    // The completion hop is the one that parks the step (`Waiting →
    // Running`): poll until the aggregate says so.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let snapshot = loop {
        let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");
        if state_of(&snapshot, "s1")
            == crate::scheduler::core::model::task_graph::TaskState::Running
        {
            break snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the kernel step never resumed running: {:?}",
            snapshot
                .tasks
                .iter()
                .map(|(id, task)| format!("{}:{:?}", id.node.as_str(), task.state))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the kernel step runs while its branch runs"
    );
    assert_eq!(
        state_of(&snapshot, crate::utils::consts::TASK_ROOT_TID),
        crate::scheduler::core::model::task_graph::TaskState::Running,
        "the kernel root runs while its children run"
    );

    // Completing the branch settles the run.
    kernel_engine
        .runtime()
        .do_action(&crate::event::Action::new(
            &pid,
            &branch,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the branch act must complete");
    let host = kernel_engine.runtime().kernel().clone();
    let settled = wait_kernel_settled(&host, PID).await;
    assert_eq!(
        state_of(&settled, crate::utils::consts::TASK_ROOT_TID),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the root completes once the branch settled"
    );
    assert_eq!(
        state_of(&settled, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
    );
    kernel_engine.close().await;
}

/// Run the timeout workflow on one engine the way a client would: complete
/// the init act, watch the branch acts' messages until both windows opened,
/// then complete the owner's act and wait for the run to settle. The branch
/// acts' messages are the observable both engines share on this path (the
/// legacy tick's intermediate skipped instances are its own).
async fn run_timeout_engine(pid: &str, workflow: &crate::model::Workflow) -> crate::Engine {
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let runtime = engine.runtime().clone();
    let wait_for_created = |key: &'static str| {
        let sig = engine.signal((String::new(), String::new()));
        let (done, done_rx) = (sig.clone(), sig.clone());
        engine.channel().on_message(move |e| {
            let done = done_rx.clone();
            async move {
                if e.is_params_key(key) && e.is_state(crate::MessageState::Created) {
                    done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                    done.close();
                }
            }
        });
        done
    };
    let init_created = wait_for_created("init");
    let act1_created = wait_for_created("act1");
    let fired = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    {
        let fired = fired.clone();
        engine.channel().on_message(move |e| {
            let fired = fired.clone();
            async move {
                for key in ["msg1", "msg2"] {
                    if !e.is_params_key(key) {
                        continue;
                    }
                    let mut fired = fired.lock();
                    if !fired.iter().any(|seen| seen == key) {
                        fired.push(key.to_string());
                    }
                }
            }
        });
    }
    runtime
        .start(
            workflow,
            crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
        )
        .await
        .unwrap();
    let (pid_now, tid) =
        tokio::time::timeout(std::time::Duration::from_secs(30), init_created.recv())
            .await
            .expect("the init act never appeared")
            .clone();
    runtime
        .do_action(&crate::event::Action::new(
            &pid_now,
            &tid,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the init act must complete");
    let (pid_now, act1) =
        tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
            .await
            .expect("the act1 act never appeared")
            .clone();
    // The owner waits on its act; the ticks fire the declared windows in
    // order, so the second branch only appears after its own window opened.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    while fired.lock().len() < 2 {
        if std::time::Instant::now() >= deadline {
            let host = engine.runtime().kernel().clone();
            let snapshot = host.snapshot(pid).await.unwrap().expect("the aggregate");
            let dump = format!(
                "executions={:?} operations={:?} owner_vars={:?}",
                snapshot
                    .tasks
                    .iter()
                    .map(|(id, task)| format!("{id:?}:{:?}", task.state))
                    .collect::<Vec<_>>(),
                snapshot
                    .operations
                    .values()
                    .map(|operation| format!(
                        "{:?}:{:?}:{:?}",
                        operation.id, operation.kind, operation.state
                    ))
                    .collect::<Vec<_>>(),
                snapshot
                    .variables
                    .iter()
                    .filter(|((scope, _), _)| scope.node.as_str() == "s1")
                    .map(|((_, name), value)| format!("{name}={value}"))
                    .collect::<Vec<_>>(),
            );
            panic!(
                "the timeout branches never fired: {:?}; {dump}",
                fired.lock()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // Both branches fired and settled: the owner's act completes, and the
    // step with it.
    runtime
        .do_action(&crate::event::Action::new(
            &pid_now,
            &act1,
            crate::event::EventAction::Next,
            crate::Vars::new(),
        ))
        .await
        .expect("the act1 act must complete");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let settled = engine
            .runtime()
            .kernel()
            .process_completed(pid)
            .await
            .unwrap();
        if settled {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the run never settled"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    engine
}

/// The timeout branches of a step fire from the tick, one declaration-ordered
/// round at a time, their window the cost since the step started. The second
/// window opens a tick after the first, and the step completes only once its
/// act is done *and* the branches it fired settled, which is what lets the run
/// end.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_timeout_branches_fire_and_settle() {
    const PID: &str = "p_timeout_parity";
    let workflow = || {
        crate::model::Workflow::new()
            .with_id("wf-timeout")
            .with_step(|step| {
                step.with_id("s0").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "init"),
                )
            })
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "act1"),
                    )
                    .with_timeout(|timeout| {
                        timeout.with_id("t1").with_if("$cost() >= 300").with_uses(
                            crate::utils::test::USES_MSG,
                            crate::Vars::new().with("key", "msg1"),
                        )
                    })
                    .with_timeout(|timeout| {
                        timeout.with_id("t2").with_if("$cost() >= 1500").with_uses(
                            crate::utils::test::USES_MSG,
                            crate::Vars::new().with("key", "msg2"),
                        )
                    })
            })
            .with_step(|step| step.with_id("s2"))
    };

    let kernel_engine = run_timeout_engine(PID, &workflow()).await;
    let host = kernel_engine.runtime().kernel().clone();
    let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");

    for node in ["t1", "t2"] {
        assert_eq!(
            state_of(&snapshot, node),
            crate::scheduler::core::model::task_graph::TaskState::Completed,
            "the branch '{node}' must have fired and settled"
        );
    }
    kernel_engine.close().await;
}

// ---- the effect seam: Func packages run inside the ApplyAction hop ----

/// Every `uses` step writes a durable `message.created` record when it
/// starts — the legacy act's Created message: the Irq step waits on it
/// (Waiting), the Func step pairs it with its completion.
#[tokio::test]
async fn uses_steps_write_created_message_records() {
    // A registry with one Func and one Irq package, so the run_as
    // decisions match the shapes under test.
    struct LocalFuncPackage;
    #[async_trait::async_trait]
    impl crate::package::ActPackage for LocalFuncPackage {
        fn new(_: &crate::Config) -> crate::Result<Self> {
            Ok(Self)
        }
        fn definition() -> crate::package::ActPackageDefinition {
            crate::package::ActPackageDefinition {
                id: "test.func",
                name: "Func",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: serde_json::json!({}),
                options: None,
                run_as: crate::package::ActRunAs::Func,
                resources: Vec::new(),
                catalog: crate::package::ActPackageCatalog::App,
            }
        }
    }
    struct LocalIrqPackage;
    #[async_trait::async_trait]
    impl crate::package::ActPackage for LocalIrqPackage {
        fn new(_: &crate::Config) -> crate::Result<Self> {
            Ok(Self)
        }
        fn definition() -> crate::package::ActPackageDefinition {
            crate::package::ActPackageDefinition {
                id: "test.irq",
                name: "Irq",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: serde_json::json!({}),
                options: None,
                run_as: crate::package::ActRunAs::Irq,
                resources: Vec::new(),
                catalog: crate::package::ActPackageCatalog::App,
            }
        }
    }
    let package = crate::package::Package::default();
    package.register(
        "test.func",
        &crate::package::ActPackageRegister::new::<LocalFuncPackage>(),
    );
    package.register(
        "test.irq",
        &crate::package::ActPackageRegister::new::<LocalIrqPackage>(),
    );
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

    let (host, mut wake_rx) = KernelHost::in_memory();
    let workflow = crate::model::Workflow::new()
        .with_step(|step| {
            step.with_id("s1")
                .with_uses("test.func", crate::Vars::new().with("k1", "v1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses("test.irq", crate::Vars::new().with("key", "irq-1"))
        });
    // The transport is attached before the run: a hop delivers its own records
    // inside its commit, so nothing waits for a pass to see them.
    struct TopicTransport {
        topics: Arc<parking_lot::Mutex<Vec<String>>>,
    }
    impl crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport for TopicTransport {
        fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome {
            self.topics.lock().push(record.topic.clone());
            DeliveryOutcome::Delivered {
                receipt: "ok".into(),
            }
        }
    }
    let topics = Arc::new(parking_lot::Mutex::new(Vec::new()));
    host.attach_outbox_transport(Box::new(TopicTransport {
        topics: topics.clone(),
    }));
    let spec = ProcessSpec::from_workflow("p-msg", &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot("p-msg").await.unwrap().unwrap();
    let message_of = |nid: &str| {
        snapshot
            .outbox
            .values()
            .find(|record| {
                record.topic == "message.created"
                    && serde_json::from_str::<serde_json::Value>(&record.payload)
                        .ok()
                        .and_then(|payload| {
                            payload
                                .get("nid")
                                .and_then(|nid| nid.as_str())
                                .map(String::from)
                        })
                        .as_deref()
                        == Some(nid)
            })
            .unwrap_or_else(|| panic!("no message.created record for '{nid}'"))
    };
    let s1_message = message_of("s1");
    let s1_payload: serde_json::Value = serde_json::from_str(&s1_message.payload).unwrap();
    assert_eq!(s1_payload["uses"], "test.func");
    assert_eq!(s1_payload["kind"], "act");
    assert_eq!(s1_payload["inputs"]["params"]["k1"], "v1");
    let s2_message = message_of("s2");
    let s2_payload: serde_json::Value = serde_json::from_str(&s2_message.payload).unwrap();
    assert_eq!(s2_payload["uses"], "test.irq");
    assert_eq!(s2_payload["kind"], "act");
    assert_eq!(s2_payload["inputs"]["params"]["key"], "irq-1");

    // The Irq execution is parked on its message; the plain leaf behind
    // it waits for the client.
    let state = |task: &str| state_of(&snapshot, task);
    assert_eq!(
        state("s2"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    assert_eq!(
        state("s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );

    // The records reached the transport with the commits that wrote them: the
    // pass has nothing left to deliver.
    assert_eq!(host.dispatch_outbox().await.unwrap(), 0);
    let delivered = topics.lock().clone();
    assert_eq!(
        delivered
            .iter()
            .filter(|topic| *topic == "message.created")
            .count(),
        3,
        "two uses steps and the workflow root announced themselves: {delivered:?}"
    );
    // The root's own message is task-shaped (kind = workflow), the acts'
    // carry their package.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&message_of("$").payload).unwrap()["kind"],
        "workflow"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&message_of("$").payload)
            .unwrap()
            .get("uses"),
        None
    );
    // A second pass delivers nothing: every record was confirmed.
    let redelivered = host.dispatch_outbox().await.unwrap();
    assert_eq!(redelivered, 0, "delivered records are not redelivered");
}

/// Effect params resolve `$`-expressions against the acting execution's
/// scope variables, exactly like a legacy task's params.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn effect_params_resolve_scope_variables() {
    let engine = crate::Engine::builder().start().await.unwrap();
    let (host, mut wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

    let workflow = crate::model::Workflow::new()
        .with_var("v", 5)
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("n", "${{ v }}"),
            )
        });
    let spec = ProcessSpec::from_workflow("p-params", &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot("p-params").await.unwrap().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    assert_eq!(
        snapshot
            .variables
            .get(&(s1.clone(), "n".to_string()))
            .map(String::as_str),
        Some("5"),
        "the params expression must resolve the workflow variable"
    );
    engine.close().await;
}

/// The whole message path: a Func step's package runs, its completion
/// record and the Irq step's Created record both forward through
/// [`message::MessageForwarder`] as engine messages.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn outbox_records_forward_as_engine_messages() {
    let engine = crate::Engine::builder().start().await.unwrap();
    let (host, mut wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let collected: Arc<parking_lot::Mutex<Vec<crate::Message>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = collected.clone();
    host.attach_outbox_transport(Box::new(message::MessageForwarder::new(
        move |msg: &crate::Message| sink.lock().push(msg.clone()),
    )));
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

    let workflow = crate::model::Workflow::new()
        .with_id("wf-msg-e2e")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("k1", "v1").with("n", 2),
            )
        })
        .with_step(|step| {
            step.with_id("s2").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "irq-1"),
            )
        });
    let spec = ProcessSpec::from_workflow("p-msg-e2e", &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.dispatch_outbox().await.unwrap();

    let messages = collected.lock().clone();
    let created = messages
        .iter()
        .find(|msg| msg.is_irq() && msg.state == crate::MessageState::Created)
        .expect("the Irq step's Created message must be forwarded");
    assert_eq!(created.pid, "p-msg-e2e");
    assert_eq!(created.tid, tid_of("p-msg-e2e", "s2", 0));
    assert_eq!(created.mid, "wf-msg-e2e");
    assert_eq!(
        created.inputs.get::<serde_json::Value>("params").unwrap()["key"],
        "irq-1"
    );
    let completed = messages
        .iter()
        .find(|msg| msg.state == crate::MessageState::Completed && msg.nid == "s1")
        .expect("the Func step's Completed message must be forwarded");
    assert_eq!(completed.outputs.get::<i32>("n"), Some(2));
    // The event channels key on the act's params key, exactly like the
    // legacy Created message.
    assert!(collect_subscriber_sees(&messages, "irq-1"));
    engine.close().await;
}

/// The forwarded messages are what a channel subscriber would filter on:
/// `is_params_key` reads the params on the act's Created message.
fn collect_subscriber_sees(messages: &[crate::Message], params_key: &str) -> bool {
    messages.iter().any(|msg| {
        msg.state == crate::MessageState::Created
            && msg
                .inputs
                .get::<serde_json::Value>("params")
                .and_then(|params| {
                    params
                        .get("key")
                        .and_then(|value| value.as_str())
                        .map(String::from)
                })
                .as_deref()
                == Some(params_key)
    })
}

/// A failing Func package, registered per-test.
struct FailingPackage;

#[async_trait::async_trait]
impl crate::package::ActPackage for FailingPackage {
    fn new(_: &crate::Config) -> crate::Result<Self> {
        Ok(Self)
    }
    fn definition() -> crate::package::ActPackageDefinition {
        crate::package::ActPackageDefinition {
            id: "test.fail",
            name: "Fail",
            desc: "always errors",
            icon: "",
            doc: "",
            version: "0.1.0",
            schema: serde_json::json!({}),
            options: None,
            run_as: crate::package::ActRunAs::Func,
            resources: Vec::new(),
            catalog: crate::package::ActPackageCatalog::App,
        }
    }
    async fn execute(
        &self,
        _ctx: &crate::Context,
        _params: &serde_json::Value,
    ) -> crate::Result<Option<crate::Vars>> {
        Err(crate::ActError::Package("boom".into()))
    }
}

/// A Func step's package runs inside its completion hop: the outputs land
/// on the acting execution's scope in the same atomic commit.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn func_package_runs_inside_the_completion_hop() {
    let engine = crate::Engine::builder().start().await.unwrap();
    let (host, mut wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    // The auto/waiting decision reads the same registry the engine does.
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

    let workflow = crate::model::Workflow::new()
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("k1", "v1").with("n", 2),
            )
        })
        .with_step(|step| step.with_id("s2"));
    let spec = ProcessSpec::from_workflow("p-effect", &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    let report = host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert_eq!(report.retried, 0, "the effect must land: {report:?}");
    assert!(host.process_completed("p-effect").await.unwrap());

    let snapshot = host.snapshot("p-effect").await.unwrap().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    assert_eq!(
        snapshot
            .variables
            .get(&(s1.clone(), "k1".to_string()))
            .map(String::as_str),
        Some("\"v1\""),
        "the package outputs are scope variables of the acting execution"
    );
    assert_eq!(
        snapshot
            .variables
            .get(&(s1.clone(), "n".to_string()))
            .map(String::as_str),
        Some("2")
    );
    let state = |task: &str| state_of(&snapshot, task);
    assert_eq!(
        state("s1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state("s2"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    engine.close().await;
}

/// A package error decides the execution Failed with the error variables
/// and the error walk takes over (upstream Failed, downstream Skipped).
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn package_error_fails_the_task_and_walks_up() {
    let engine = crate::Engine::builder().start().await.unwrap();
    engine.runtime().package().register(
        "test.fail",
        &crate::package::ActPackageRegister::new::<FailingPackage>(),
    );
    // The effect runner resolves the package through its durable metadata
    // row (like the legacy engine's `package_definition`), so the failure
    // under test is the package's own.
    engine
        .executor(&crate::Principal::unrestricted())
        .ext()
        .register_package(&<FailingPackage as crate::package::ActPackage>::definition())
        .await
        .unwrap();
    let (host, mut wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

    let workflow = crate::model::Workflow::new()
        .with_step(|step| step.with_id("s0"))
        .with_step(|step| {
            step.with_id("s1")
                .with_uses("test.fail", crate::Vars::new())
        })
        .with_step(|step| step.with_id("s2"));
    let spec = ProcessSpec::from_workflow("p-effect-fail", &workflow, &run_as).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

    assert!(host.process_completed("p-effect-fail").await.unwrap());
    let snapshot = host.snapshot("p-effect-fail").await.unwrap().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    assert_eq!(
        snapshot
            .variables
            .get(&(s1.clone(), "error.code".to_string()))
            .map(String::as_str),
        Some("EPACKAGE")
    );
    assert!(
        snapshot
            .variables
            .contains_key(&(s1.clone(), "error.message".to_string()))
    );
    let state = |task: &str| state_of(&snapshot, task);
    assert_eq!(
        state("s1"),
        crate::scheduler::core::model::task_graph::TaskState::Failed
    );
    // The error walk: the ancestor chain ends Failed, the untouched
    // downstream skips.
    assert_eq!(
        state("$"),
        crate::scheduler::core::model::task_graph::TaskState::Failed
    );
    assert_eq!(
        state("s0"),
        crate::scheduler::core::model::task_graph::TaskState::Completed
    );
    assert_eq!(
        state("s2"),
        crate::scheduler::core::model::task_graph::TaskState::Skipped
    );
    engine.close().await;
}

/// `scope_vars` is the variable space an effect's params resolve against:
/// the chain of the acting execution, with the engine's own aggregate
/// facts (`__model`, `__auto`, `__owner`) left out — a legacy task never
/// reads them.
#[test]
fn scope_vars_carry_the_run_variables_without_the_engine_facts() {
    let workflow = crate::model::Workflow::new()
        .with_var("v", 5)
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow("p-vars", &workflow, &|_| None).unwrap();
    let snapshot = spec.seed_snapshot().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    let names: Vec<String> = scope_vars(&snapshot, &s1)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(names.contains(&"v".to_string()), "{names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("__")),
        "engine facts must not be variables: {names:?}"
    );
}

/// Effect params seal snapshot data through the legacy task-init path: the
/// headless scope carries the run's owner, derives the target's scope key
/// from the acting execution's variables, seals under the owner's
/// authority, and only then fills `${{ }}`. The sealed values are pinned as
/// the frozen golden the legacy parity run produced (the legacy engine was
/// run on the same store with the same owner and snapshot).
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn effect_seals_snapshot_params() {
    let engine = crate::Engine::builder().start().await.unwrap();
    engine
        .add_snapshot(
            "profile",
            crate::SnapshotOptions {
                scope: vec!["uid".to_string()],
                ..crate::SnapshotOptions::per_task()
            },
        )
        .unwrap();
    engine
        .snapshot()
        .upsert(
            "profile",
            "u1",
            1,
            crate::Vars::new().with("secrets", crate::Vars::new().with("TOKEN", "sk-1")),
        )
        .unwrap();
    let owner = crate::ScopePolicy {
        subject: "u1".to_string(),
        scopes: std::collections::HashMap::from([(
            "profile".to_string(),
            vec!["$subject".to_string()],
        )]),
        ..crate::ScopePolicy::deny_all()
    };

    // The scope key comes from the run's start inputs, the way a real
    // `proc:start` hands its `uid` in — not from a declared workflow var.
    let start_vars = crate::Vars::new().with("uid", "u1");
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("s1").with_uses(
            crate::utils::test::USES_SET,
            crate::Vars::new()
                .with("token", "${{ $profile.secrets.TOKEN }}")
                .with("uid", "${{ uid }}"),
        )
    });

    // The kernel side: the effect runs the package inside the completion
    // hop, and its outputs (the resolved params) land on the execution.
    let (host, mut wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let spec = ProcessSpec::from_workflow("p-seal", &workflow, &run_as)
        .unwrap()
        .with_owner(owner.clone())
        .with_vars(&start_vars);
    host.start_process(&spec).await.unwrap();
    let report = host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert_eq!(report.retried, 0, "the seal must land: {report:?}");
    assert!(host.process_completed("p-seal").await.unwrap());
    let snapshot = host.snapshot("p-seal").await.unwrap().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    let kernel = |name: &str| {
        snapshot
            .variables
            .get(&(s1.clone(), name.to_string()))
            .unwrap_or_else(|| panic!("effect output '{name}' missing"))
            .clone()
    };
    let kernel_token = kernel("token");
    let kernel_uid = kernel("uid");

    // The frozen golden from the legacy parity run this test used to compare
    // against: the act was handed the *sealed* snapshot value and the run's
    // start input, and the aggregate stores their JSON encoding.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&kernel_token).unwrap(),
        serde_json::json!("sk-1"),
        "the sealed snapshot value must be what the package received"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&kernel_uid).unwrap(),
        serde_json::json!("u1"),
        "the sealed scope key comes from the run's start inputs"
    );
    engine.close().await;
}

/// The seal is the owner's: a run whose authority does not own the scope
/// reads nothing and its effect fails where the legacy task would, rather
/// than leaking another subject's data.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn effect_denies_a_snapshot_scope_the_owner_lacks() {
    let engine = crate::Engine::builder().start().await.unwrap();
    engine
        .add_snapshot(
            "profile",
            crate::SnapshotOptions {
                scope: vec!["uid".to_string()],
                ..crate::SnapshotOptions::per_task()
            },
        )
        .unwrap();
    engine
        .snapshot()
        .upsert(
            "profile",
            "u1",
            1,
            crate::Vars::new().with("secrets", crate::Vars::new().with("TOKEN", "sk-1")),
        )
        .unwrap();
    // The pattern names its own subject, which is not the scope key.
    let intruder = crate::ScopePolicy {
        subject: "intruder".to_string(),
        scopes: std::collections::HashMap::from([(
            "profile".to_string(),
            vec!["$subject".to_string()],
        )]),
        ..crate::ScopePolicy::deny_all()
    };

    // The scope key resolves from the run's start inputs (see the parity
    // test), so the seal is what the authority fails on — not a missing
    // key.
    let workflow = crate::model::Workflow::new().with_step(|step| {
        step.with_id("s1").with_uses(
            crate::utils::test::USES_SET,
            crate::Vars::new().with("token", "${{ $profile.secrets.TOKEN }}"),
        )
    });

    let (host, mut wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let spec = ProcessSpec::from_workflow("p-denied", &workflow, &run_as)
        .unwrap()
        .with_owner(intruder)
        .with_vars(&crate::Vars::new().with("uid", "u1"));
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    assert!(host.process_completed("p-denied").await.unwrap());
    let snapshot = host.snapshot("p-denied").await.unwrap().unwrap();
    let s1 = crate::scheduler::core::model::task_graph::TaskId::new("s1", 0).unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Failed
    );
    let error = snapshot
        .variables
        .get(&(s1.clone(), "error.message".to_string()))
        .expect("the denied seal fails the effect")
        .clone();
    assert!(error.contains("not owned"), "unexpected error: {error}");
    assert!(
        !snapshot.variables.contains_key(&(s1, "token".to_string())),
        "a denied seal must not produce outputs"
    );
    engine.close().await;
}

/// A commit that writes outbox records delivers them at once: the operation's
/// dispatch runs the targeted post-commit pass instead of waiting for the tick.
///
/// The tick here is an hour away, so a message can only have come from that
/// targeted pass — and a client's next action waits for exactly this message,
/// which is why the tick cadence (15 s by default) turned every client-driven
/// step into a tick-long stall. The tick stays the retry net.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_commit_delivers_its_records_without_waiting_for_a_tick() {
    let engine = crate::Engine::builder().start().await.unwrap();
    let (host, wake_rx) = KernelHost::in_memory();
    host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
        engine.runtime().clone(),
    )));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::Message>();
    host.attach_outbox_transport(Box::new(message::MessageForwarder::new(
        move |msg: &crate::Message| {
            let _ = tx.send(msg.clone());
        },
    )));

    let package = engine.runtime().package().clone();
    let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
    let workflow = crate::model::Workflow::new()
        .with_id("wf-immediate-delivery")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "irq-1"),
            )
        });
    let spec = ProcessSpec::from_workflow("p-immediate-delivery", &workflow, &run_as).unwrap();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let driver = tokio::spawn(host.clone().serve(
        wake_rx,
        shutdown.clone(),
        super::KernelSchedule::uniform(std::time::Duration::from_secs(3600)),
    ));
    host.start_process(&spec).await.unwrap();

    let created = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(message) = rx.recv().await {
            if message.is_irq() && message.state == crate::MessageState::Created {
                return message;
            }
        }
        panic!("the delivery channel closed before the act was announced");
    })
    .await
    .expect("the act's Created message must arrive without a tick");
    assert_eq!(created.pid, "p-immediate-delivery");
    assert_eq!(created.tid, tid_of("p-immediate-delivery", "s1", 0));
    assert_eq!(
        created.inputs.get::<serde_json::Value>("params").unwrap()["key"],
        "irq-1"
    );

    shutdown.cancel();
    driver.await.unwrap();
    engine.close().await;
}

/// A store that instruments the aggregate batches of one run: it counts the
/// plan commits that add outbox records and the follow-up confirmations that
/// settle them, can refuse one plan commit, can lose one confirmation's
/// guard, and records every key written (for ordering assertions).
struct InstrumentedKv {
    inner: MemoryStore,
    process_key: String,
    writes: parking_lot::Mutex<Vec<String>>,
    fail_plan_commit: AtomicBool,
    plan_commit_failures: AtomicUsize,
    fail_confirmation: AtomicBool,
    confirmation_failures: AtomicUsize,
    plan_commits_with_records: AtomicUsize,
    added_records: AtomicUsize,
    confirmations: AtomicUsize,
    confirmed_records: AtomicUsize,
    max_confirmation_batch: AtomicUsize,
}

impl InstrumentedKv {
    fn new(process_id: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            process_key: crate::scheduler::durable::document_store::process_key(process_id)
                .expect("a process id the aggregate key derives from"),
            writes: parking_lot::Mutex::new(Vec::new()),
            fail_plan_commit: AtomicBool::new(false),
            plan_commit_failures: AtomicUsize::new(0),
            fail_confirmation: AtomicBool::new(false),
            confirmation_failures: AtomicUsize::new(0),
            plan_commits_with_records: AtomicUsize::new(0),
            added_records: AtomicUsize::new(0),
            confirmations: AtomicUsize::new(0),
            confirmed_records: AtomicUsize::new(0),
            max_confirmation_batch: AtomicUsize::new(0),
        })
    }

    fn arm_plan_commit_failure(&self) {
        self.fail_plan_commit.store(true, Ordering::SeqCst);
    }

    fn arm_confirmation_failure(&self) {
        self.fail_confirmation.store(true, Ordering::SeqCst);
    }

    fn plan_commit_failures(&self) -> usize {
        self.plan_commit_failures.load(Ordering::SeqCst)
    }

    fn confirmation_failures(&self) -> usize {
        self.confirmation_failures.load(Ordering::SeqCst)
    }

    fn plan_commits_with_records(&self) -> usize {
        self.plan_commits_with_records.load(Ordering::SeqCst)
    }

    fn added_records(&self) -> usize {
        self.added_records.load(Ordering::SeqCst)
    }

    fn confirmations(&self) -> usize {
        self.confirmations.load(Ordering::SeqCst)
    }

    fn confirmed_records(&self) -> usize {
        self.confirmed_records.load(Ordering::SeqCst)
    }

    fn max_confirmation_batch(&self) -> usize {
        self.max_confirmation_batch.load(Ordering::SeqCst)
    }

    fn wrote_prefix(&self, prefix: &str) -> bool {
        self.writes.lock().iter().any(|key| key.starts_with(prefix))
    }

    fn remember(&self, ops: &[crate::StoreBatchOp]) {
        let mut writes = self.writes.lock();
        for op in ops {
            match op {
                crate::StoreBatchOp::Put { key, .. } | crate::StoreBatchOp::Delete { key } => {
                    writes.push(key.clone());
                }
            }
        }
    }

    fn replacement_snapshot(&self, ops: &[crate::StoreBatchOp]) -> Option<ProcessSnapshot> {
        ops.iter().find_map(|op| match op {
            crate::StoreBatchOp::Put { key, value } if key == &self.process_key => {
                crate::scheduler::durable::document::ProcessDocumentCodec::decode(value).ok()
            }
            _ => None,
        })
    }

    async fn stored_snapshot(&self) -> Option<ProcessSnapshot> {
        self.inner
            .one(&self.process_key)
            .await
            .ok()
            .flatten()
            .and_then(|bytes| {
                crate::scheduler::durable::document::ProcessDocumentCodec::decode(&bytes).ok()
            })
    }
}

#[async_trait::async_trait]
impl KvStore for InstrumentedKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.writes.lock().push(key.to_string());
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.writes.lock().push(key.to_string());
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[crate::StoreBatchOp],
        guards: &[crate::StoreGuard],
    ) -> Result<bool> {
        let writes_process = ops.iter().any(
            |op| matches!(op, crate::StoreBatchOp::Put { key, .. } if key == &self.process_key),
        );
        let (old, new) = if writes_process {
            (self.stored_snapshot().await, self.replacement_snapshot(ops))
        } else {
            (None, None)
        };
        if let (Some(old), Some(new)) = (&old, &new) {
            let confirmation = old.version == new.version && old.outbox != new.outbox;
            if confirmation && self.fail_confirmation.swap(false, Ordering::SeqCst) {
                self.confirmation_failures.fetch_add(1, Ordering::SeqCst);
                // A guard that does not hold: the batch applies nothing.
                return Ok(false);
            }
            let plan_commit = old.version != new.version
                && old
                    .operations
                    .values()
                    .any(|operation| matches!(operation.state, OperationState::CommitPrepared(_)));
            if plan_commit && self.fail_plan_commit.swap(false, Ordering::SeqCst) {
                self.plan_commit_failures.fetch_add(1, Ordering::SeqCst);
                return Err(crate::ActError::Store(
                    "injected plan commit failure".into(),
                ));
            }
        }
        let applied = self.inner.batch(ops, guards).await?;
        if applied {
            self.remember(ops);
            if let (Some(old), Some(new)) = (old, new) {
                if old.version == new.version {
                    let changed = new
                        .outbox
                        .iter()
                        .filter(|(id, record)| old.outbox.get(*id) != Some(record))
                        .count();
                    if changed > 0 {
                        self.confirmations.fetch_add(1, Ordering::SeqCst);
                        self.confirmed_records.fetch_add(changed, Ordering::SeqCst);
                        self.max_confirmation_batch
                            .fetch_max(changed, Ordering::SeqCst);
                    }
                } else {
                    let added = new
                        .outbox
                        .keys()
                        .filter(|id| !old.outbox.contains_key(*id))
                        .count();
                    if added > 0 {
                        self.plan_commits_with_records
                            .fetch_add(1, Ordering::SeqCst);
                        self.added_records.fetch_add(added, Ordering::SeqCst);
                    }
                }
            }
        }
        Ok(applied)
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: crate::ScanOptions,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// A transport that checks, at delivery time, whether the read model's proc
/// row was already written: the message must never precede the projection.
struct OrderingTransport {
    kv: Arc<InstrumentedKv>,
    proc_prefix: String,
    observed: Arc<parking_lot::Mutex<Vec<bool>>>,
}

impl crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport for OrderingTransport {
    fn deliver(&mut self, _record: &OutboxRecord) -> DeliveryOutcome {
        self.observed
            .lock()
            .push(self.kv.wrote_prefix(&self.proc_prefix));
        DeliveryOutcome::Delivered {
            receipt: "ordered".into(),
        }
    }
}

fn two_record_plan(process_id: &str, snapshot: &ProcessSnapshot) -> TransitionPlan {
    TransitionPlan {
        process_id: process_id.into(),
        expected: snapshot.version.clone(),
        task_creates: Vec::new(),
        state_changes: Vec::new(),
        // An outbox-only plan does not move the aggregate version; the marker
        // makes this a real transition, exactly like an operation's plan.
        variable_writes: vec![
            crate::scheduler::core::model::transition_plan::VariableWrite {
                scope: crate::scheduler::core::model::task_graph::TaskId::new("root", 0).unwrap(),
                name: "batch.marker".into(),
                value: "1".into(),
            },
        ],
        source_operation: None,
        operations: Vec::new(),
        outbox: vec![
            crate::scheduler::core::model::transition_plan::OutboxIntent {
                key: "batch:first".into(),
                topic: "batch.first".into(),
                payload: "{}".into(),
            },
            crate::scheduler::core::model::transition_plan::OutboxIntent {
                key: "batch:second".into(),
                topic: "batch.second".into(),
                payload: "{}".into(),
            },
        ],
    }
}

/// A message is not visible before its aggregate commit: when the plan commit
/// is refused, the transport is never called and no record reaches the
/// aggregate.
#[tokio::test]
async fn an_outbox_message_waits_for_its_aggregate_commit() {
    const PID: &str = "p-outbox-commit";
    let stored = InstrumentedKv::new(PID);
    stored.arm_plan_commit_failure();
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let delivered = Arc::new(parking_lot::Mutex::new(0));
    host.attach_outbox_transport(Box::new(CountingTransport {
        delivered: delivered.clone(),
    }));
    let spec = ProcessSpec::linear(PID, "root", &["only"]);
    host.start_process(&spec).await.unwrap();

    let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert_eq!(stored.plan_commit_failures(), 1);
    assert!(
        report.retried >= 1,
        "the refused commit must surface: {report:?}"
    );
    assert_eq!(
        *delivered.lock(),
        0,
        "a failed plan commit must not reach the transport"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        snapshot.outbox.is_empty(),
        "a failed plan commit writes no record: {:?}",
        snapshot.outbox
    );
}

/// The normal path settles a whole operation's records in **one** confirmation
/// commit: every plan commit that adds records is followed by exactly one
/// aggregate write that advances them, and records are never claimed or
/// confirmed one at a time.
#[tokio::test]
async fn one_operation_confirms_all_its_records_in_one_commit() {
    const PID: &str = "p-outbox-batch";
    let stored = InstrumentedKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let delivered = Arc::new(parking_lot::Mutex::new(0));
    host.attach_outbox_transport(Box::new(CountingTransport {
        delivered: delivered.clone(),
    }));
    let spec = ProcessSpec::linear(PID, "root", &["a", "b"]);
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    assert_eq!(
        stored.confirmations(),
        stored.plan_commits_with_records(),
        "one confirmation per operation, never one per record"
    );
    assert_eq!(
        stored.confirmed_records(),
        stored.added_records(),
        "every committed record was confirmed"
    );
    assert_eq!(
        *delivered.lock(),
        stored.added_records(),
        "the normal path delivers each record exactly once"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        snapshot
            .outbox
            .values()
            .all(|record| matches!(record.state, OutboxState::Delivered { .. })),
        "the normal path leaves no pending record: {:?}",
        snapshot.outbox
    );
}

/// One operation carrying several records still pays a single confirmation
/// commit, and that commit advances all of them.
#[tokio::test]
async fn a_commit_with_two_records_confirms_them_in_one_commit() {
    const PID: &str = "p-outbox-two";
    let stored = InstrumentedKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, _wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let delivered = Arc::new(parking_lot::Mutex::new(0));
    host.attach_outbox_transport(Box::new(CountingTransport {
        delivered: delivered.clone(),
    }));
    let spec = ProcessSpec::linear(PID, "root", &["only"]);
    host.start_process(&spec).await.unwrap();
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let plan = two_record_plan(PID, &snapshot);
    let (receipt, mut committed) = match host
        .processes
        .commit_transition(&plan, SystemClock.now_ms(), host.timer_poll_ms())
        .await
        .unwrap()
    {
        crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Committed {
            receipt,
            loaded,
        }
        | crate::scheduler::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted {
            receipt,
            loaded,
        } => (receipt, loaded),
        crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
            panic!("the plan commit must apply")
        }
    };
    assert_eq!(receipt.new_outbox.len(), 2);

    host.deliver_committed_outbox(&mut committed, &receipt.new_outbox)
        .await;

    assert_eq!(
        stored.confirmations(),
        1,
        "two records, one confirmation commit"
    );
    assert_eq!(stored.max_confirmation_batch(), 2);
    assert_eq!(*delivered.lock(), 2);
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        snapshot
            .outbox
            .values()
            .all(|record| matches!(record.state, OutboxState::Delivered { .. }))
    );
    assert!(
        host.processes
            .load(PID)
            .await
            .unwrap()
            .unwrap()
            .original_outbox_index()
            .is_none(),
        "delivered records leave no outbox index entry"
    );
}

/// A confirmation that loses its aggregate guard loses no message: the
/// records stay `Pending`, the outbox pass redelivers them, and the run
/// settles. The redelivery is the documented at-least-once duplicate.
#[tokio::test]
async fn a_lost_confirmation_is_redelivered_without_losing_the_message() {
    const PID: &str = "p-outbox-confirm-conflict";
    let stored = InstrumentedKv::new(PID);
    stored.arm_confirmation_failure();
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let delivered = Arc::new(parking_lot::Mutex::new(0));
    host.attach_outbox_transport(Box::new(CountingTransport {
        delivered: delivered.clone(),
    }));
    let spec = ProcessSpec::linear(PID, "root", &["only"]);
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    assert_eq!(stored.confirmation_failures(), 1);
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let pending: Vec<_> = snapshot
        .outbox
        .values()
        .filter(|record| matches!(record.state, OutboxState::Pending { .. }))
        .collect();
    assert_eq!(
        pending.len(),
        1,
        "the lost confirmation leaves its record pending: {:?}",
        snapshot.outbox
    );
    let sent_once = *delivered.lock();
    assert!(sent_once >= 1);

    let settled = host.dispatch_outbox().await.unwrap();
    assert_eq!(settled, 1, "the retry pass redelivers the pending record");
    assert_eq!(
        *delivered.lock(),
        sent_once + 1,
        "at-least-once: the lost confirmation's record is sent again"
    );
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert!(
        snapshot
            .outbox
            .values()
            .all(|record| matches!(record.state, OutboxState::Delivered { .. })),
        "the retry pass settles the message: {:?}",
        snapshot.outbox
    );
}

/// Partial transport outcomes persist through the same one batch without
/// losing the record's semantics, and A11 keeps the run because its outbox is
/// not all `Delivered`.
#[tokio::test]
async fn partial_delivery_failures_are_persisted_and_keep_the_run() {
    const PID: &str = "p-outbox-partial";
    struct PartialTransport {
        delivered: Arc<parking_lot::Mutex<usize>>,
    }
    impl crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport for PartialTransport {
        fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome {
            *self.delivered.lock() += 1;
            match record.topic.as_str() {
                "task.started" => DeliveryOutcome::RetryAt {
                    available_at_ms: SystemClock.now_ms() + 60_000,
                    error: "busy".into(),
                },
                "task.completed" => DeliveryOutcome::OutcomeUnknown {
                    reason: "transport lost the ack".into(),
                },
                _ => DeliveryOutcome::Quarantine {
                    reason: "refused".into(),
                },
            }
        }
    }
    let kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES, 800);
    let delivered = Arc::new(parking_lot::Mutex::new(0));
    host.attach_outbox_transport(Box::new(PartialTransport {
        delivered: delivered.clone(),
    }));
    let spec = ProcessSpec::linear(PID, "root", &["only"]);
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(*delivered.lock() > 0);

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let has = |predicate: &dyn Fn(&OutboxState) -> bool| {
        snapshot
            .outbox
            .values()
            .any(|record| predicate(&record.state))
    };
    assert!(
        has(
            &|state| matches!(state, OutboxState::Pending { available_at_ms } if *available_at_ms > 0)
        ),
        "RetryAt must persist as a deferred pending record: {:?}",
        snapshot.outbox
    );
    assert!(
        has(&|state| matches!(state, OutboxState::DeliveryUnknown { .. })),
        "an unclassified outcome must persist as DeliveryUnknown: {:?}",
        snapshot.outbox
    );
    assert!(
        has(&|state| matches!(state, OutboxState::Quarantined { .. })),
        "a quarantine must persist: {:?}",
        snapshot.outbox
    );
    assert!(!snapshot.is_reclaimable(), "A11 must retain this run");
    let loaded = host.processes.load(PID).await.unwrap().unwrap();
    let index = crate::scheduler::core::outbox::outbox_index::OutboxIndexCodec::decode(
        loaded
            .original_outbox_index()
            .expect("undelivered records keep the outbox index"),
    )
    .unwrap();
    use crate::scheduler::core::outbox::outbox_index::OutboxIndexState;
    assert!(
        index.entries.iter().any(|entry| matches!(
            entry.state,
            OutboxIndexState::Pending { available_at_ms } if available_at_ms > 0
        )),
        "the index carries the deferred retry: {:?}",
        index.entries
    );
    assert!(
        index
            .entries
            .iter()
            .any(|entry| matches!(entry.state, OutboxIndexState::DeliveryUnknown)),
        "the index carries the unknown delivery: {:?}",
        index.entries
    );
    assert!(
        index
            .entries
            .iter()
            .all(|entry| !matches!(entry.state, OutboxIndexState::Claimed { .. })),
        "a confirmed loss is not left claimed: {:?}",
        index.entries
    );
    assert!(
        matches!(
            host.reclaim_process(PID).await.unwrap(),
            crate::scheduler::durable::async_document_store::ReclaimOutcome::Retained
        ),
        "an undelivered record keeps the aggregate"
    );
}

/// The transport runs after the projection: every message observes the proc
/// row of its commit already written.
#[tokio::test]
async fn messages_are_visible_only_after_the_projection() {
    const PID: &str = "p-outbox-projection";
    let stored = InstrumentedKv::new(PID);
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv.clone(), 1, MAX_NODE_RUN_TIMES, 800);
    let store = Arc::new(crate::store::Store::new(kv));
    let writer = crate::store::writebehind::StoreWriter::spawn(
        store.clone(),
        1,
        64,
        crate::utils::shared(1),
    );
    host.attach_row_projector(Arc::new(RowProjector::new(store, writer, None)));
    let observed = Arc::new(parking_lot::Mutex::new(Vec::new()));
    host.attach_outbox_transport(Box::new(OrderingTransport {
        kv: stored.clone(),
        proc_prefix: crate::store::StoreIden::Procs.as_ref().to_string(),
        observed: observed.clone(),
    }));
    let workflow = crate::model::Workflow::new()
        .with_id("wf-outbox-projection")
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let observed = observed.lock().clone();
    assert!(!observed.is_empty(), "the run must have delivered messages");
    assert!(
        observed.iter().all(|written| *written),
        "a message saw the proc row missing: {observed:?}"
    );
}

/// A store that injects a newer row between an older conditional write's read
/// and its guarded batch: on the first guarded batch touching `key`, it stores
/// `value` under that key (the write a second instance would have committed),
/// then lets the older batch run. The guard must refuse it, and the projector
/// must re-read and report the row stale. This is the one interleaving the
/// in-process document lock cannot produce; the store guard is what covers it.
struct GatedKv {
    inner: MemoryStore,
    gate: parking_lot::Mutex<Option<(String, Vec<u8>)>>,
    entered: tokio::sync::Notify,
}

impl GatedKv {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            gate: parking_lot::Mutex::new(None),
            entered: tokio::sync::Notify::new(),
        })
    }

    fn arm(&self, key: String, value: Vec<u8>) {
        *self.gate.lock() = Some((key, value));
    }

    async fn wait_entered(&self) {
        self.entered.notified().await;
    }
}

#[async_trait::async_trait]
impl KvStore for GatedKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[crate::StoreBatchOp],
        guards: &[crate::StoreGuard],
    ) -> Result<bool> {
        let inject = {
            let mut gate = self.gate.lock();
            match gate.as_ref() {
                Some((key, _))
                    if ops.iter().any(
                        |op| matches!(op, crate::StoreBatchOp::Put { key: written, .. } if written == key),
                    ) =>
                {
                    gate.take()
                }
                _ => None,
            }
        };
        if let Some((key, value)) = inject {
            // A concurrent instance's newer projection lands first.
            self.inner.put(&key, value).await?;
            self.entered.notify_one();
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: crate::store::ScanOptions,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// A store that refuses every guarded batch touching a read-model row while
/// armed — an outage of the projection path, not of the aggregate.
struct FailingRowsKv {
    inner: MemoryStore,
    armed: AtomicBool,
}

impl FailingRowsKv {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            armed: AtomicBool::new(false),
        })
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }
}

fn is_row_key(key: &str) -> bool {
    ["procs", "tasks", "vars"]
        .iter()
        .any(|prefix| key.starts_with(&format!("{prefix}-")))
}

#[async_trait::async_trait]
impl KvStore for FailingRowsKv {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[crate::StoreBatchOp],
        guards: &[crate::StoreGuard],
    ) -> Result<bool> {
        if self.armed.load(Ordering::SeqCst)
            && ops.iter().any(|op| match op {
                crate::StoreBatchOp::Put { key, .. } | crate::StoreBatchOp::Delete { key } => {
                    is_row_key(key)
                }
            })
        {
            return Err(crate::ActError::Store(
                "injected projection row failure".into(),
            ));
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: crate::store::ScanOptions,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// A5: the durable per-row revision, not the in-process claim table, decides.
///
/// The instrumented store commits a newer row between the older projection's
/// read and its guarded batch — the interleaving the in-process document lock
/// cannot produce and a second instance can. The old batch's guard must fail,
/// the re-read must see the newer revision, and the row must stay newer.
#[tokio::test(flavor = "multi_thread")]
async fn an_older_projection_cannot_overwrite_a_newer_one() {
    const PID: &str = "p-projection-cas";
    let gated = GatedKv::new();
    let kv: Arc<dyn KvStore> = gated.clone();
    let store = Arc::new(crate::store::Store::new(kv));
    let writer = crate::store::writebehind::StoreWriter::spawn(
        store.clone(),
        1,
        64,
        crate::utils::shared(1),
    );

    let workflow = crate::model::Workflow::new()
        .with_id("wf-projection-cas")
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    let base = spec.seed_snapshot().unwrap();
    let root = base
        .tasks
        .keys()
        .find(|id| id.node.as_str() == crate::utils::consts::TASK_ROOT_TID)
        .expect("the seed's root")
        .clone();
    let mut mid = base.clone();
    mid.version.process += 1;
    mid.tasks.get_mut(&root).unwrap().state = TaskState::Running;
    let mut new = base.clone();
    new.version.process += 2;
    new.tasks.get_mut(&root).unwrap().state = TaskState::Completed;

    // The baseline row (version 1.0.0).
    RowProjector::new(store.clone(), writer.clone(), None)
        .project(&base)
        .await
        .unwrap();
    let wire = root.wire(PID);
    let row_id = crate::utils::Id::new(PID, &wire).id();
    let base_row = store.tasks().find_opt(&row_id).await.unwrap().unwrap();
    assert_eq!(base_row.source_revision, base.version.encode());

    // The row a concurrent, newer projection would have committed.
    let mut newer = base_row.clone();
    newer.source_revision = new.version.encode();
    newer.state = "completed".into();
    gated.arm(
        format!("tasks-id-{row_id}"),
        serde_json::to_vec(&newer).unwrap(),
    );

    // A fresh projector has no claim for this row: it reads the baseline and
    // reaches its guarded batch. The store writes `newer` first, then runs
    // the older guarded batch (which must fail).
    let probe = RowProjector::new(store.clone(), writer.clone(), None);
    let report = probe.project(&mid).await.unwrap();
    assert!(
        report.stale > 0,
        "the older projection must be refused by the store guard: {report:?}"
    );
    gated.wait_entered().await;

    let row = store.tasks().find_opt(&row_id).await.unwrap().unwrap();
    assert_eq!(row.source_revision, new.version.encode());
    assert_eq!(row.state, "completed", "the older row must not win");

    // A restart has no in-process claims at all: the persisted revision is
    // still enough to refuse the older snapshot.
    let restarted = RowProjector::new(store.clone(), writer, None);
    restarted.project(&mid).await.unwrap();
    let row = store.tasks().find_opt(&row_id).await.unwrap().unwrap();
    assert_eq!(row.source_revision, new.version.encode());
    assert_eq!(row.state, "completed");
    assert!(
        restarted.metrics().stale > 0,
        "with no claim table the store guard is the only guard"
    );
}

/// A5: a restart re-projects from the persisted revisions — a row that lagged
/// behind its aggregate is repaired by the boot catch-up; a row already at the
/// aggregate's version is left unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_catches_up_only_the_rows_that_lag() {
    const PID: &str = "p-projection-restart";
    let kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let store = Arc::new(crate::store::Store::new(kv.clone()));
    let writer = crate::store::writebehind::StoreWriter::spawn(
        store.clone(),
        1,
        64,
        crate::utils::shared(1),
    );
    let (host, mut wake_rx) = KernelHost::new(kv.clone(), 1, MAX_NODE_RUN_TIMES, 800);
    host.attach_row_projector(Arc::new(RowProjector::new(
        store.clone(),
        writer.clone(),
        None,
    )));
    let workflow = crate::model::Workflow::new()
        .with_id("wf-projection-restart")
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    let current = snapshot.version.encode();
    let s1_id = crate::utils::Id::new(PID, &tid_of(PID, "s1", 0)).id();
    let mut row = store.tasks().find_opt(&s1_id).await.unwrap().unwrap();
    assert_eq!(row.source_revision, current);
    // Simulate a crash between a commit and its projection: the row is stale
    // and reads a state the aggregate left behind.
    row.state = "running".into();
    row.source_revision = "1.0.0".into();
    store.tasks().update(&row).await.unwrap();

    // A fresh host over the same store: no in-process projection claims.
    let (restarted, _wake) = KernelHost::new(kv.clone(), 1, MAX_NODE_RUN_TIMES, 800);
    restarted.attach_row_projector(Arc::new(RowProjector::new(store.clone(), writer, None)));
    restarted.recover(SystemClock.now_ms()).await.unwrap();

    let row = store.tasks().find_opt(&s1_id).await.unwrap().unwrap();
    assert_eq!(
        row.source_revision, current,
        "the boot catch-up must restore the row's source revision"
    );
    assert_eq!(row.state, "completed");
    let metrics = restarted.projection_metrics();
    assert_eq!(
        metrics.lag, 0,
        "a successful catch-up leaves no repair queued"
    );
    assert!(
        metrics.repaired > 0,
        "the boot catch-up counts the run it repaired: {metrics:?}"
    );
}

/// A5: a projection failure is queued, withholds the completion event, and is
/// repaired by the recovery pass — after which the observer sees the row.
#[tokio::test(flavor = "multi_thread")]
async fn a_projection_failure_is_repaired_before_the_completion_event() {
    const PID: &str = "p-projection-repair";
    let stored = FailingRowsKv::new();
    let kv: Arc<dyn KvStore> = stored.clone();
    let store = Arc::new(crate::store::Store::new(kv.clone()));
    let writer = crate::store::writebehind::StoreWriter::spawn(
        store.clone(),
        1,
        64,
        crate::utils::shared(1),
    );
    let (host, mut wake_rx) = KernelHost::new(kv.clone(), 1, MAX_NODE_RUN_TIMES, 800);
    host.attach_row_projector(Arc::new(RowProjector::new(store.clone(), writer, None)));
    let completed = Arc::new(AtomicUsize::new(0));
    let seen = completed.clone();
    host.attach_lifecycle_sink(Box::new(move |_, event| {
        if event == ProcessEvent::Completed {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    }));
    // Take the boot pass before the rows exist, so the later repair goes
    // through the repair queue rather than the boot catch-up.
    host.recover(SystemClock.now_ms()).await.unwrap();

    stored.arm();
    let workflow = crate::model::Workflow::new()
        .with_id("wf-projection-repair")
        .with_step(|step| step.with_id("s1"));
    let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
    host.start_process(&spec).await.unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert_eq!(
        completed.load(Ordering::SeqCst),
        0,
        "a completion event must not overtake its rows"
    );
    let metrics = host.projection_metrics();
    assert!(
        metrics.failed > 0,
        "the failure must be counted: {metrics:?}"
    );
    assert!(
        metrics.lag > 0,
        "the run must be queued for repair: {metrics:?}"
    );

    stored.disarm();
    // A10: projection repair is its own loop, not part of the recovery scan.
    assert_eq!(host.repair_projections().await, 0);
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "the repaired projection releases the withheld completion event"
    );
    let metrics = host.projection_metrics();
    assert!(metrics.repaired > 0, "{metrics:?}");
    assert_eq!(metrics.lag, 0, "{metrics:?}");
    let proc = store.procs().find_opt(PID).await.unwrap().unwrap();
    let aggregate = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(proc.source_revision, aggregate.version.encode());
}

/// A5: the query API is a strong read. A proc row left behind its aggregate
/// (a failed projection, a crash between commit and projection) is repaired
/// before the row is served, and the returned process reflects the aggregate.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn a_proc_read_reprojects_a_lagging_row() {
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    let workflow = crate::model::Workflow::new()
        .with_id("wf-strong-read")
        .with_step(|step| step.with_id("s1"));
    let runtime = engine.runtime();
    let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
    let pid = proc.id().to_string();
    tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
        .await
        .expect("the run never finished");

    let snapshot = runtime.kernel().snapshot(&pid).await.unwrap().unwrap();
    let current = snapshot.version.encode();
    let mut row = runtime
        .store()
        .procs()
        .find_opt(&pid)
        .await
        .unwrap()
        .unwrap();
    row.state = "running".into();
    row.source_revision = "1.0.0".into();
    runtime.store().procs().update(&row).await.unwrap();

    let live = runtime.proc(&pid).await.unwrap().expect("the proc row");
    assert_eq!(
        live.state(),
        crate::LegacyTaskState::Completed,
        "the strong read must serve the aggregate's state"
    );
    let restored = runtime
        .store()
        .procs()
        .find_opt(&pid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.source_revision, current,
        "the strong read must restore the row's source revision"
    );
    assert_eq!(runtime.projection_metrics().lag, 0);
    engine.close().await;
}
