mod health;
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
    // 3 nodes x (execute + action + propagate) hops, root included.
    assert_eq!(report.executed, 9, "nine hops expected: {report:?}");
    assert_eq!(report.retried, 0);
    assert_all_terminal(&host, "p1").await;

    let snapshot = host.snapshot("p1").await.unwrap().unwrap();
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
async fn wake_loss_is_recovered_by_the_scan() {
    let (host, mut wake_rx) = KernelHost::in_memory();
    let spec = ProcessSpec::linear("p2", "root", &["only"]);
    // Create without waking: everything now rides on the recovery scan,
    // exactly like after a crash that lost the in-memory queue.
    let snapshot = spec.seed_snapshot().unwrap();
    host.processes
        .create(&snapshot)
        .await
        .map_err(runtime)
        .unwrap();
    let now = SystemClock.now_ms();
    let report = host.recover(now).await.unwrap();
    assert!(
        !report.woken.is_empty(),
        "the pending root operation must be re-derived: {report:?}"
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
    operation: OperationId,
    armed: AtomicBool,
    parks: AtomicUsize,
    parked: tokio::sync::Notify,
    released: parking_lot::Mutex<bool>,
    open: tokio::sync::Notify,
}

impl ConflictKv {
    /// A store ready to park `operation`'s plan commit in `process_id`'s
    /// aggregate — armed from construction, so no hop can slip past before the
    /// owning test has driven it.
    fn new(process_id: &str, operation: OperationId) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            process_key: crate::scheduler::durable::document_store::process_key(process_id)
                .expect("a process id the aggregate key derives from"),
            operation,
            armed: AtomicBool::new(true),
            parks: AtomicUsize::new(0),
            parked: tokio::sync::Notify::new(),
            released: parking_lot::Mutex::new(false),
            open: tokio::sync::Notify::new(),
        })
    }

    /// Whether this batch is the hop's plan commit: it writes the aggregate
    /// while the hop's own operation already stands `CommitPrepared`, i.e. the
    /// claim landed and the plan is about to.
    async fn is_plan_commit(&self, ops: &[crate::StoreBatchOp]) -> bool {
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
                .get(&self.operation)
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
    let stored = ConflictKv::new(PID, root.clone());
    let kv: Arc<dyn KvStore> = stored.clone();
    let (host, mut wake_rx) = KernelHost::new(kv, 1, MAX_NODE_RUN_TIMES);
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
        host.processes.commit_transition(&plan).await.unwrap();
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

    let report = host.recover(SystemClock.now_ms()).await.unwrap();
    assert!(
        report.woken.contains(&root),
        "the released hop must be re-derived by the recovery scan: {report:?}"
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
    let (host, mut wake_rx) = KernelHost::new(kv.clone(), 1, MAX_NODE_RUN_TIMES);
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

    // The only slot is `cap-a`'s: four of its six hops run here (its fifth
    // settles the run), and the two runs behind it never move.
    let report = host.run_until_quiescent(&mut wake_rx, 4).await.unwrap();
    assert_eq!(
        report.executed, 4,
        "only the admitted run may hop: {report:?}"
    );
    assert_eq!(*started.lock(), vec!["cap-a".to_string()]);
    assert!(!host.process_completed("cap-a").await.unwrap());
    for pid in ["cap-b", "cap-c"] {
        assert_parked(&host, &store, pid).await;
    }

    // The remaining hops of `cap-a` settle it; the freed slot admits `cap-b`,
    // whose settlement admits `cap-c` — each one immediately, without waiting
    // for a tick.
    let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    assert_eq!(
        report.executed, 14,
        "two hops left of `cap-a` plus six each for the two admitted runs: {report:?}"
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
        .find_opt(&crate::utils::Id::new(pid, crate::utils::consts::TASK_ROOT_TID).id())
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
        host.processes.create(&snapshot).await.unwrap();
        seeds.push((pid.to_string(), snapshot));
    }
    let report = host.recover(SystemClock.now_ms()).await.unwrap();
    assert_eq!(report.woken.len(), 2, "{report:?}");
    let first = report.woken[0].process_id.clone();
    let second = report.woken[1].process_id.clone();

    // The scan wakes both runs: the one woken first takes the only slot, the
    // one behind it is offered work and waits.
    let report = host.run_until_quiescent(&mut wake_rx, 4).await.unwrap();
    assert!(
        report.ignored >= 1,
        "the run over the cap must be offered work and wait: {report:?}"
    );
    assert!(!host.process_completed(&first).await.unwrap());
    let waiting = host.snapshot(&second).await.unwrap().expect("aggregate");
    let seeded = &seeds
        .iter()
        .find(|(pid, _)| pid == &second)
        .expect("the second wake belongs to a seeded run")
        .1;
    assert_eq!(
        waiting, *seeded,
        "a run waiting for a slot has not hopped: its aggregate is still its seed"
    );

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
        .with_step(|step| {
            step.with_id("s1").with_uses(
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
    assert_eq!(
        tasks,
        vec![crate::utils::consts::TASK_ROOT_TID.to_string(), "s1".into()]
    );
    // The effect's output is the task scope's projected data.
    let s1 = executor.task().get(&pid, "s1").await.expect("the task row");
    assert_eq!(s1.state, "completed");
    assert_eq!(s1.r#type, "step");
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
        .filter(|row| row.tid == "s1")
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
    // run is sweepable.
    for row in &messages.rows {
        assert_eq!(
            row.status, "completed",
            "every delivery of a finished run must be settled: {:?}",
            messages.rows
        );
    }
    runtime.sweep_removable().await.unwrap();
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

    std::fs::remove_dir_all(&root).ok();
    engine.close().await;
}

/// Restart discrimination: a pid's durable record is the aggregate, so a
/// same-pid start is refused both while the projected rows exist and after
/// the sweeper reclaimed them — the aggregate outlives the rows.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn kernel_engine_refuses_a_started_pid_via_the_process() {
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
        .with_id("wf-dup")
        .with_step(|step| step.with_id("s1"));
    let runtime = engine.runtime();
    let pid = "duppid";
    let options = crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid);
    runtime.start(&workflow, options.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
        .await
        .expect("the run never finished");

    let duplicated = runtime.start(&workflow, options.clone()).await;
    assert!(
        duplicated.is_err_and(|err| err.to_string().contains("duplicated")),
        "an existing pid must be refused while its rows are there"
    );

    // Settle and sweep the rows: the aggregate is what still answers.
    let host = runtime.kernel().clone();
    host.dispatch_outbox().await.unwrap();
    runtime.sweep_removable().await.unwrap();
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
    assert!(host.exists(pid).await.unwrap(), "the aggregate stays");
    let duplicated = runtime.start(&workflow, options).await;
    assert!(
        duplicated.is_err_and(|err| err.to_string().contains("duplicated")),
        "the aggregate must refuse a restart the swept rows no longer show"
    );

    engine.close().await;
}

/// The package write-back: what a Func package leaves in its context —
/// the returned outputs, the task data it wrote, and the process env it
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
            ctx.task()
                .update_data(&crate::Vars::new().with("written", 7));
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
        // refused (the rows would then read the state before it). Wait for the
        // run's own row to read terminal before collecting the set.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            runtime.flush_writer().await.unwrap();
            let state = runtime
                .store()
                .procs()
                .find_opt(&pid)
                .await
                .unwrap()
                .map(|row| row.state);
            if state
                .as_deref()
                .is_some_and(|state| state != "running" && state != "none")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run's row never read terminal: {state:?}"
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
            "s1",
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
        .apply_action("p-cancel-guard", "s1", ActionKind::Cancel, Vec::new())
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
    host.apply_action("p-cancel-guard", "s1", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action("p-cancel-guard", "s2", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    assert!(host.process_completed("p-cancel-guard").await.unwrap());
    let settled = host
        .apply_action("p-cancel-guard", "s1", ActionKind::Cancel, Vec::new())
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
    host.apply_action("p-cancel-lone", "s1", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    let empty = host
        .apply_action("p-cancel-lone", "s1", ActionKind::Cancel, Vec::new())
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
    // The kernel's read model is the surviving generation per node: the
    // redone s1 completed, the redone s2 waits again (the act's state, the
    // one the legacy *act* row also shows), and no backed/completed
    // history — the row is the instance the read API addresses.
    assert_eq!(
        states_of(&kernel, "s2"),
        vec!["interrupted".to_string()],
        "the row is the surviving generation, not the history"
    );
    assert_eq!(states_of(&kernel, "s1"), vec!["completed".to_string()]);
    // The root diverges and is registered in the migration notes: the
    // kernel completes the workflow root at its own hop, so a redo leaves
    // it (and the projected process row) `completed` while the run is still
    // in flight, where the legacy root waits for every child instance.
    // Asserting it here would pin that divergence, so this test stops at
    // the nodes both engines agree on.
    assert_eq!(
        kernel
            .iter()
            .filter(|row| row.tid == crate::utils::consts::TASK_ROOT_TID)
            .count(),
        1,
        "one row per node: the root"
    );
    assert_eq!(
        kernel.iter().filter(|row| row.tid == "s1").count(),
        1,
        "one row per node: the redone s1"
    );
    assert_eq!(
        kernel.iter().filter(|row| row.tid == "s2").count(),
        1,
        "one row per node: the redone s2"
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
            "the kernel run never settled: {:?}",
            snapshot
                .tasks
                .iter()
                .map(|(id, task)| format!("{}:{:?}", id.node.as_str(), task.state))
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
    host.apply_action("p4", "wait", ActionKind::Complete, Vec::new())
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
    host.recover(SystemClock.now_ms()).await.unwrap();
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
    host.recover(SystemClock.now_ms()).await.unwrap();
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
    host.apply_action(PID, "s1", ActionKind::Complete, Vec::new())
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
    host.apply_action(PID, "t1", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    host.recover(SystemClock.now_ms()).await.unwrap();
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
    host.apply_action(PID, "a", ActionKind::Complete, Vec::new())
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
        "s1",
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
    host.recover(SystemClock.now_ms()).await.unwrap();
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
    host.apply_action(PID, "s1", ActionKind::Complete, Vec::new())
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
    host.apply_action(PID, "t1", ActionKind::Complete, Vec::new())
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
        .apply_action("p5", "$", ActionKind::Complete, Vec::new())
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
    host.apply_action("p6", "head", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
    // The rewind: head gen0 keeps the legacy act's `Backed` fact, the
    // chain forward of it is skipped, and a fresh head generation is
    // already parked waiting for its own action.
    host.apply_action("p6", "head", ActionKind::Cancel, Vec::new())
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
    host.apply_action("p6", "head", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action("p6", "mid", ActionKind::Complete, Vec::new())
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
    host.snapshot(pid).await.unwrap().unwrap()
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

    host.apply_action("p-wait", "s11", ActionKind::Complete, Vec::new())
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
        "step1",
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
    host.apply_action("p-cancel-early", "step1", ActionKind::Complete, Vec::new())
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

    host.apply_action("p-cancel-early", "step1", ActionKind::Cancel, Vec::new())
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
    host.apply_action("p-cancel-early", "step1", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    host.apply_action("p-cancel-early", "step2", ActionKind::Complete, Vec::new())
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
    host.apply_action("p-cancel-mid", "s11", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
    let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "step1"),
        crate::scheduler::core::model::task_graph::TaskState::Completed,
        "the container settled once its branches did"
    );

    host.apply_action("p-cancel-mid", "step1", ActionKind::Cancel, Vec::new())
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

    host.apply_action("p-cancel-mid", "s11", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();
    host.apply_action("p-cancel-mid", "step2", ActionKind::Complete, Vec::new())
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
    host.processes.commit_transition(&intent).await.unwrap();
    assert_eq!(
        state_of(&host.snapshot(PID).await.unwrap().unwrap(), "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Waiting,
        "the intent alone decides nothing"
    );

    host.recover(SystemClock.now_ms()).await.unwrap();
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
        }],
        outbox: Vec::new(),
    };
    bare_host.processes.commit_transition(&bare).await.unwrap();
    bare_host.recover(SystemClock.now_ms()).await.unwrap();
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
/// atomic commit closes the source generation, pins the target fact and
/// creates the `ReenterTask` operation whose hop plans the fresh
/// generations. Recovery alone drives that hop — the property that makes
/// the rewind survive a lost compare-and-swap or a crash between the
/// decision and the re-entry (the host used to plan it inline, after the
/// commit, where both failures lost it).
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
    host.apply_action(PID, "s1", ActionKind::Complete, Vec::new())
        .await
        .unwrap();
    host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
    host.apply_action(PID, "s1", ActionKind::Cancel, Vec::new())
        .await
        .unwrap();
    // Durable before any hop ran: the decision closed the source, pinned
    // the rewind target and created the operation that plans it.
    let snapshot = host.snapshot(PID).await.unwrap().unwrap();
    assert_eq!(
        state_of(&snapshot, "s1"),
        crate::scheduler::core::model::task_graph::TaskState::Backed
    );
    let source = crate::scheduler::core::model::task_graph::TaskId {
        node: crate::scheduler::core::model::model_graph::NodeId::parse("s1").unwrap(),
        generation: 0,
    };
    assert_eq!(
        snapshot
            .variables
            .get(&(source.clone(), REENTER_TARGET_VAR.to_string()))
            .map(String::as_str),
        Some("s1"),
        "the deciding commit pins the target the rewind re-enters"
    );
    let rewind = snapshot
        .operations
        .iter()
        .find(|(_, operation)| operation.kind == OperationKind::ReenterTask)
        .map(|(id, operation)| (id.clone(), operation.state.clone()));
    let (rewind_id, rewind_state) = rewind.expect("the deciding commit creates the rewind");
    assert!(
        matches!(
            rewind_state,
            crate::scheduler::core::model::operation::OperationState::Pending
        ),
        "the rewind starts pending: recovery owns it until it commits, got {rewind_state:?}"
    );

    // No wake survives from the deciding call: the rewind below is driven
    // by the recovery scan, exactly like a rewind whose deciding call died
    // before it could dispatch anything.
    while wake_rx.try_recv().is_ok() {}
    host.recover(SystemClock.now_ms()).await.unwrap();
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
    let rewind = snapshot
        .operations
        .get(&rewind_id)
        .expect("the rewind exists");
    assert!(
        matches!(
            rewind.state,
            crate::scheduler::core::model::operation::OperationState::Committed(_)
                | crate::scheduler::core::model::operation::OperationState::Completed(_)
        ),
        "the rewind hop carried its own commit, got {:?}",
        rewind.state
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

    host.apply_action("p-abort", "s2", ActionKind::Abort, Vec::new())
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

    host.apply_action("p-abort-branch", "s11", ActionKind::Abort, Vec::new())
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
        "s2",
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

    host.apply_action("p-back", "s2", ActionKind::Complete, Vec::new())
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
        .apply_action("p-back-no-to", "s1", ActionKind::Back, Vec::new())
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
        "s2",
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
    host.apply_action("k-abort", "s2", ActionKind::Abort, Vec::new())
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
        "s2",
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
    assert_eq!(s1_payload["inputs"]["params"]["k1"], "v1");
    let s2_message = message_of("s2");
    let s2_payload: serde_json::Value = serde_json::from_str(&s2_message.payload).unwrap();
    assert_eq!(s2_payload["uses"], "test.irq");
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
    assert_eq!(created.tid, "s2");
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

/// A commit that writes outbox records delivers them at once: the driver's
/// delivery pass is woken by the commit itself instead of waiting for the tick.
///
/// The tick here is an hour away, so a message can only have come from that
/// wake — and a client's next action waits for exactly this message, which is
/// why the tick cadence (15 s by default) turned every client-driven step into
/// a tick-long stall. The tick stays the retry net.
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
        std::time::Duration::from_secs(3600),
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
    assert_eq!(created.tid, "s1");
    assert_eq!(
        created.inputs.get::<serde_json::Value>("params").unwrap()["key"],
        "irq-1"
    );

    shutdown.cancel();
    driver.await.unwrap();
    engine.close().await;
}
