//! The outage and stall cells: a store that goes away mid-run, and the
//! recovery a later engine has to perform from whatever the durable state was
//! left in at a cut.
//!
//! The kernel decides a client action in three commits against the aggregate —
//! the durable *intent* (the operation plus the private action fact its operation
//! replays), the claim that prepares it, and the effect commit, which closes
//! the operation in the same batch — and projects the rows from those commits
//! afterwards. A crash is therefore a *cut*: the writer stops landing writes at
//! exactly one boundary, and whatever the aggregate holds at that boundary is
//! the state a later engine boots onto. Every cell here cuts the run's own
//! *commit* stream (a file-local store), never its raw write stream: a
//! completion's commits interleave with the row projections and outbox
//! deliveries of earlier work, so a raw write counter would land on a delivery
//! — whose fenced claim then waits out its own lease — and the cell would
//! assert the wrong boundary. The commit boundaries are the intent, the claim,
//! the effect, and the propagation the completion seeds. The runtime's own
//! passes must then finish the run: on demand with no restart, on the periodic
//! tick, and across a second restart.
//!
//! Two kernel facts shape the cells, and each one says where it matters:
//!
//! - the kernel keeps its outbox records *inside* the aggregate, so an operation
//!   either commits its record with its source operation or leaves nothing —
//!   there is no open `next` row a writer could lose and re-materialize. What
//!   legacy's abandoned record is here is a durable *operation* left
//!   non-terminal: an operation that ended without closing it, which the recovery
//!   scan is what re-drives;
//! - the read model is eventual and nothing repairs a *settled* run's rows, so
//!   a cell reads "exactly once" and "no stranded work" off the aggregate
//!   (its executions, its operations and its outbox records) and checks the
//!   rows only through the sweeper — reading a run that an abandoned delivery
//!   claim holds as settled-but-unreclaimed, the delivery lifecycle's own hole.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

use serial_test::serial;

use crate::LegacyTaskState;
use crate::scheduler::core::commit::aggregate::ProcessSnapshot;
use crate::scheduler::core::model::clock::{Clock, SystemClock};
use crate::scheduler::core::model::operation::OperationState;
use crate::scheduler::core::model::task_graph::TaskState;
use crate::scheduler::core::outbox::records::OutboxState;

use super::*;

/// The commits of one client action's operation, in order: the durable intent (the
/// operation plus its action fact), the claim that prepares it, and the effect.
/// `CLAIM_WRITE` is the commit the strand cells let the intent precede and then
/// refuse — the intent is the commit before it. The effect commit is also the
/// close (the operation's `Committed` → `Completed` runs inside it).
const CLAIM_WRITE: usize = 2;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Start one one-step irq run on `engine` and answer `(pid, "s1")` once its act
/// waits — the state a completion is aimed at.
///
/// The `Created` message is deliberately not awaited: the act's *state* is
/// wake-driven, and a cell that reads the tid off the row (the `uses` step's
/// own node) needs no message. The wait is polled read-only — a test thread
/// that writes the aggregate while an operation is in flight can lose that operation's
/// commit to a revision conflict — and the run is only quiesced (its outbox
/// delivered) once it is quiet.
async fn waiting_run(engine: &Engine) -> (String, String) {
    let pid = start_run(engine, &one_step_irq("act1")).await;
    let what = format!("the irq act of {pid} to wait");
    poll_until_dumped(
        &what,
        || async {
            flush(engine).await;
            proc_row(engine, &pid)
                .await
                .is_some_and(|proc| waiting_task(&proc, "s1").is_some())
        },
        || dump_state(engine, &pid),
    )
    .await;
    quiesce(engine).await;
    (pid, "s1".to_string())
}

/// [`waiting_run`] under the caller's own pid: the strand cells pin the
/// aggregate document of a *known* run, so their cut can name it.
async fn waiting_run_named(engine: &Engine, pid: &str) -> (String, String) {
    let started = start_run_with(
        engine,
        &one_step_irq("act1"),
        Vars::new().with(utils::consts::PROCESS_ID, pid),
    )
    .await;
    assert_eq!(
        started, pid,
        "the run must take the pid the store was built for"
    );
    let what = format!("the irq act of {pid} to wait");
    poll_until_dumped(
        &what,
        || async {
            flush(engine).await;
            proc_row(engine, pid)
                .await
                .is_some_and(|proc| waiting_task(&proc, "s1").is_some())
        },
        || dump_state(engine, pid),
    )
    .await;
    quiesce(engine).await;
    (started, "s1".to_string())
}

/// Drain every due outbox record and the write-behind queue until a pass finds
/// nothing left. A cut armed after this lands on the run's own commits, not on
/// a delivery a tick happened to make.
async fn quiesce(engine: &Engine) {
    for _ in 0..32 {
        let delivered = engine
            .runtime()
            .kernel()
            .dispatch_outbox()
            .await
            .unwrap_or(0);
        flush(engine).await;
        if delivered == 0 {
            break;
        }
    }
}

/// Whether every execution of `pid` reached a terminal state — the run is no
/// longer in flight.
async fn completed(engine: &Engine, pid: &str) -> bool {
    engine
        .runtime()
        .kernel()
        .process_completed(pid)
        .await
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// The aggregate's own record — where "exactly once" is decided
// ---------------------------------------------------------------------------

/// The run's aggregate document. A settled run keeps it even after its rows are
/// swept, so a cell reads a finished run's decision here and never off a row.
async fn process_snapshot(engine: &Engine, pid: &str) -> ProcessSnapshot {
    engine
        .runtime()
        .kernel()
        .snapshot(pid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the aggregate of {pid} must exist"))
}

/// The ascending generations the aggregate holds for one model node: how many
/// times the node was instantiated. An operation applied twice adds one.
async fn generations(engine: &Engine, pid: &str, tid: &str) -> Vec<u64> {
    let mut generations: Vec<u64> = process_snapshot(engine, pid)
        .await
        .tasks
        .keys()
        .filter(|id| id.node == tid)
        .map(|id| id.generation)
        .collect();
    generations.sort_unstable();
    generations
}

/// The aggregate's state for the newest generation of one model node.
async fn decided_state(engine: &Engine, pid: &str, tid: &str) -> Option<TaskState> {
    process_snapshot(engine, pid)
        .await
        .tasks
        .iter()
        .filter(|(id, _)| id.node == tid)
        .max_by_key(|(id, _)| id.generation)
        .map(|(_, task)| task.state)
}

/// The non-terminal operations the aggregate holds for one model node, in
/// sequence order, as short names: the durable work a cut left for recovery.
async fn open_shapes(engine: &Engine, pid: &str, tid: &str) -> Vec<&'static str> {
    let mut open: Vec<(u64, &'static str)> = process_snapshot(engine, pid)
        .await
        .operations
        .values()
        .filter(|operation| operation.id.node == tid && !operation.is_terminal())
        .map(|operation| (operation.id.sequence, shape(&operation.state)))
        .collect();
    open.sort_by_key(|(sequence, _)| *sequence);
    open.into_iter().map(|(_, shape)| shape).collect()
}

/// One operation state as a short, comparable name.
fn shape(state: &OperationState) -> &'static str {
    match state {
        OperationState::Pending => "pending",
        OperationState::RetryScheduled { .. } => "retry-scheduled",
        OperationState::Claimed(_) => "claimed",
        OperationState::CommitPrepared(_) => "prepared",
        OperationState::Committed(_) => "committed",
        OperationState::Completed(_) => "completed",
        OperationState::EffectUnknown { .. } => "effect-unknown",
        OperationState::Quarantined { .. } => "quarantined",
        OperationState::Superseded { .. } => "superseded",
    }
}

/// How many outbox records of the run fall under `prefix`. An operation's key is
/// deterministic per `(pid, tid, generation, sequence)`, so an operation that applied
/// twice cannot hide behind the count.
async fn records_with_prefix(engine: &Engine, pid: &str, prefix: &str) -> usize {
    process_snapshot(engine, pid)
        .await
        .outbox
        .keys()
        .filter(|id| id.key.starts_with(prefix))
        .count()
}

/// The completion operation's key family for one act: one `action` record for the
/// decision and one `propagate` record for the operation it set off.
fn completion_records(pid: &str, tid: &str) -> (String, String) {
    (
        format!("action:{pid}:{tid}:"),
        format!("propagate:{pid}:{tid}:"),
    )
}

/// The finished run landed exactly once: one generation of the act and of the
/// root, both `Completed`, and exactly one record per operation of the decision.
async fn assert_finished_once(engine: &Engine, pid: &str, tid: &str) {
    assert!(completed(engine, pid).await, "run {pid} must settle");
    for node in [utils::consts::TASK_ROOT_TID, tid] {
        assert_eq!(
            generations(engine, pid, node).await,
            vec![0],
            "a converging cut may not instantiate {node} a second time"
        );
        assert_eq!(
            decided_state(engine, pid, node).await,
            Some(TaskState::Completed),
            "node {node} of {pid} must hold the completion"
        );
    }
    let (action, propagate) = completion_records(pid, tid);
    assert_eq!(
        records_with_prefix(engine, pid, &action).await,
        1,
        "one record for the completion operation"
    );
    assert_eq!(
        records_with_prefix(engine, pid, &propagate).await,
        1,
        "one record for the propagation the completion set off"
    );
}

/// The finished run, re-checked after a later recovery round: the decision did
/// not move — the run stays settled, one generation per node, one record per
/// operation. A retry of a stranded operation that re-applied a decision would show
/// up here as a second generation or a second record.
async fn assert_still_finished_once(engine: &Engine, pid: &str, tid: &str) {
    let (action, propagate) = completion_records(pid, tid);
    let what = format!("{pid} to stay finished exactly once");
    poll_until_dumped(
        &what,
        || async {
            completed(engine, pid).await
                && generations(engine, pid, tid).await == vec![0]
                && generations(engine, pid, utils::consts::TASK_ROOT_TID).await == vec![0]
                && records_with_prefix(engine, pid, &action).await == 1
                && records_with_prefix(engine, pid, &propagate).await == 1
        },
        || dump_state(engine, pid),
    )
    .await;
}

/// Whether nothing of the run is left undone: every execution terminal, every
/// operation closed or *surfaced for reconciliation* (`effect-unknown`), and no
/// outbox record still awaiting its first delivery (`Pending`). A record a
/// delivery pass already claimed, or one surfaced for reconciliation, is that
/// delivery's own unresolved business — never the operation work this drives — so it
/// does not keep the run from being converged (it does keep its *rows* alive;
/// see [`sweep_finished_run`]).
async fn fully_settled(engine: &Engine, pid: &str) -> bool {
    let snapshot = process_snapshot(engine, pid).await;
    snapshot.tasks.values().all(|task| task.state.is_terminal())
        && snapshot
            .outbox
            .values()
            .all(|record| !matches!(record.state, OutboxState::Pending { .. }))
        && snapshot.operations.values().all(|operation| {
            operation.is_terminal()
                || matches!(operation.state, OperationState::EffectUnknown { .. })
        })
}

/// The run's outstanding work as printable evidence: the outbox records that
/// are not delivered and every operation that is neither closed nor surfaced
/// for reconciliation. No timestamps, so a genuinely stuck state prints once.
async fn outstanding(engine: &Engine, pid: &str) -> String {
    let snapshot = process_snapshot(engine, pid).await;
    let mut out: Vec<String> = snapshot
        .outbox
        .iter()
        .filter(|(_, record)| !matches!(record.state, OutboxState::Delivered { .. }))
        .map(|(id, record)| format!("record {} -> {:?}", id.key, record.state))
        .collect();
    for (id, operation) in &snapshot.operations {
        if !operation.is_terminal() {
            out.push(format!(
                "op {task} seq{sequence} -> {state}",
                task = id.node,
                sequence = id.sequence,
                state = shape(&operation.state)
            ));
        }
    }
    if out.is_empty() {
        "nothing outstanding".to_string()
    } else {
        out.join(", ")
    }
}

/// The run's own operation work is done: every execution terminal, every operation
/// closed or surfaced for reconciliation. Outbox records may still be
/// undelivered — that is a delivery's business — but no operation is in flight, so a
/// caller may run the outbox pass without racing one.
async fn settled_work(engine: &Engine, pid: &str) -> bool {
    let snapshot = process_snapshot(engine, pid).await;
    snapshot.tasks.values().all(|task| task.state.is_terminal())
        && snapshot.operations.values().all(|operation| {
            operation.is_terminal()
                || matches!(operation.state, OperationState::EffectUnknown { .. })
        })
}

/// Whether the run has nothing left undone (see [`fully_settled`]), first
/// running the outbox pass *if the run's own operation work is already done*: never
/// while an operation is in flight, since a pass's write would race the operation's guarded
/// commit and a delivery claim that loses that race is never reclaimed. A
/// finished run's deliveries therefore land promptly instead of waiting for a
/// tick that may claim one mid-cascade.
async fn settle_run(engine: &Engine, pid: &str) -> bool {
    if settled_work(engine, pid).await {
        let _ = engine.runtime().kernel().dispatch_outbox().await;
    }
    fully_settled(engine, pid).await
}

/// Poll until the run has nothing left undone (see [`fully_settled`]), printing
/// the outstanding work (and never hanging — the deadline fails the cell).
async fn wait_fully_settled(engine: &Engine, pid: &str) {
    let what = format!("run {pid} to leave nothing undone");
    poll_until_dumped(
        &what,
        || async { settle_run(engine, pid).await },
        || outstanding(engine, pid),
    )
    .await;
}

/// Whether the engine still has work it will drive by itself: an operation
/// `Pending`/`RetryScheduled` (the next wake or tick re-drives it) or
/// `Committed` (recovery finalizes it; no operation produces that state any more — it
/// survives in a store a writer from before the close was folded into the
/// effect commit left behind). Read-only, so a cell can ask it while the
/// driver's operations are in flight — a `Claimed`/`CommitPrepared` operation
/// whose owner died is deliberately *not* drivable: only the claim's lease
/// releases it.
async fn engine_drivable(engine: &Engine, pid: &str) -> bool {
    process_snapshot(engine, pid)
        .await
        .operations
        .values()
        .any(|operation| {
            matches!(
                operation.state,
                OperationState::Pending
                    | OperationState::RetryScheduled { .. }
                    | OperationState::Committed(_)
            )
        })
}

/// Drive the recovered run to its end: the engine's own recovery re-derives
/// whatever the cut left durable and wakes the driver, and a decision the cut
/// *refused* — an act still waiting for the client — is decided by hand as its
/// retry would.
///
/// The engine's own operations are left alone while it has drivable work (an operation in
/// flight must not have a test thread write the aggregate under it). When it
/// stops with a claim a crash left behind — an operation `Claimed` or
/// `CommitPrepared` whose owner is gone — the scan is repeated "a minute
/// later", so the claim's lease has elapsed: recovery then reclaims it, retried
/// or surfaced for reconciliation, exactly as the boundary cells recover an
/// effect boundary. The poll fails, never hangs, if nothing progresses.
async fn drive_to_end(engine: &Engine, pid: &str, tid: &str) {
    let runtime = engine.runtime();
    let kernel = runtime.kernel();
    // Wake whatever is drivable now (a fresh boot holds nothing in flight).
    let _ = kernel.recover(SystemClock.now_ms()).await;

    let started = Instant::now();
    let last_nudge = Cell::new(None::<Instant>);
    let what = format!("run {pid} to finish from its durable state");
    poll_until_dumped(
        &what,
        || async {
            if settle_run(engine, pid).await {
                return true;
            }
            if !engine_drivable(engine, pid).await {
                // The engine has stopped with nothing it will drive. If it has
                // been stopped for a while, a claim from a dead owner is what
                // blocks it: reclaim it with the lease elapsed.
                let nudge_due = started.elapsed() > Duration::from_millis(1_500)
                    && last_nudge
                        .get()
                        .is_none_or(|at| at.elapsed() > Duration::from_millis(1_500));
                if nudge_due {
                    last_nudge.set(Some(Instant::now()));
                    let runtime = engine.runtime();
                    let kernel = runtime.kernel();
                    let _ = kernel.recover(SystemClock.now_ms() + 120_000).await;
                    let _ = kernel.recover(SystemClock.now_ms()).await;
                } else if let Some(proc) = proc_row(engine, pid).await
                    && waiting_task(&proc, tid).is_some()
                {
                    let _ = complete_act(engine, pid, tid).await;
                }
            }
            false
        },
        || outstanding(engine, pid),
    )
    .await;
}

/// Close `engine` and let its driver task observe the shutdown.
///
/// `Runtime::close` cancels the token and closes the writer but does not join
/// the spawned driver, so an operation the driver is inside finishes *after* `close`
/// returns — possibly writing the aggregate. A cell that reuses the store must
/// not have that write land in the middle of the next engine's operations (it loses
/// the operation's guarded commit and strands it for its claim lease), so a cell
/// waits the driver out.
async fn stop(engine: &Engine) {
    engine.close().await;
    // Long enough for an operation the driver was inside to finish *before* the store
    // is healed and handed to the next engine: a driver that outlives the
    // close would otherwise commit into the new engine's store and lose one of
    // its operations' guarded commits at random.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// Reclaim a finished run's rows and assert no row family of it is left — the
/// harness's [`sweep_until_gone`] — unless an unresolved delivery keeps them
/// alive.
///
/// The sweeper marks a run removable only once every outbox record is
/// `Delivered`. The outbox pass only ever *claims* `Pending` records (its page
/// filter is `Pending { available_at_ms }`), so a record left `Claimed` with
/// its owner gone — a crash mid-delivery, or a confirm that lost its guarded
/// write to a concurrent commit — is never re-dispatched: its run can never be
/// marked removable, and its rows stay while the run itself is settled. When
/// that is what the cell finds, it asserts the run *is* settled and names the
/// record that keeps it instead of pretending the rows were reclaimed; the hole
/// in the delivery lifecycle is the cell's finding, reported with it.
async fn sweep_finished_run(engine: &Engine, pid: &str) {
    assert!(
        completed(engine, pid).await,
        "run {pid} must be settled before its rows are reclaimed"
    );
    let reclaimed = poll_settle(
        || async {
            let _ = engine.runtime().sweep_removable().await;
            engine.runtime().store().procs().find(pid).await.is_err()
        },
        200,
    )
    .await;
    if reclaimed {
        sweep_until_gone(engine, pid).await;
        return;
    }
    let snapshot = process_snapshot(engine, pid).await;
    assert!(
        snapshot.tasks.values().all(|task| task.state.is_terminal()),
        "an unreclaimed run must still be settled: {}",
        outstanding(engine, pid).await
    );
    let unresolved: Vec<String> = snapshot
        .outbox
        .iter()
        .filter(|(_, record)| !matches!(record.state, OutboxState::Delivered { .. }))
        .map(|(id, _)| id.key.clone())
        .collect();
    if !unresolved.is_empty() {
        eprintln!(
            "NOTE: {pid} stays unreclaimed — its sweeper waits on the unresolved delivery record(s) {unresolved:?}"
        );
        return;
    }
    // Every delivery resolved and the run settled: what keeps the rows is the
    // sweeper's own `removable` flag on the proc row, stale because nothing
    // re-projects a *settled* run's row (the row is re-derived by the next
    // commit of the run, and there is none).
    let removable = engine
        .runtime()
        .store()
        .procs()
        .find(pid)
        .await
        .map(|row| row.removable)
        .unwrap_or(true);
    assert!(
        !removable,
        "a settled run with every delivery resolved must be removable: {}",
        outstanding(engine, pid).await
    );
    eprintln!(
        "NOTE: {pid} stays unreclaimed — its proc row still reads removable=false though every delivery resolved"
    );
}

/// The crash and the boot, in the order that makes the crash real: stop the
/// engine while the cut still refuses every write (no live driver can apply the
/// operation the crash left behind, and none may leak a write into the new engine),
/// then heal the store — a fresh boot writes — and open a fresh engine over the
/// same durable state.
async fn crash_and_reboot(engine: &Engine, store: Arc<dyn KvStore>, kv: &DocCutKv) -> Engine {
    stop(engine).await;
    kv.heal();
    engine_on(store).await
}

// ---------------------------------------------------------------------------
// A cut at a *commit* boundary
// ---------------------------------------------------------------------------

/// A KV backend that stops honouring *operation commits* of one run from the
/// `budget`-th one after it is armed.
///
/// The harness's [`CutKv`] counts *every* write, so a row projection or a
/// delivery landing between two of an operation's commits shifts the boundary a cut
/// lands on: the cells would cut the intent instead of the claim on some runs
/// and pass or fail by timing. An operation's commits are the ones that change the
/// run's *recovery index* (the derived document listing its non-terminal
/// operations) — an outbox delivery rewrites the same bytes — so this backend
/// spends its budget only on those and is immune to that interleave. Reads keep
/// answering (the crash is of the writer, not the backend).
struct DocCutKv {
    inner: MemoryStore,
    /// The authoritative document the budget's commits carry.
    key: String,
    /// The derived index that only an operation commit changes.
    recovery_key: String,
    budget: AtomicUsize,
    armed: AtomicBool,
    commits: AtomicUsize,
    refused: AtomicUsize,
}

impl DocCutKv {
    fn new(pid: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            key: process_key(pid),
            recovery_key: crate::scheduler::durable::document_store::recovery_index_key(pid)
                .expect("a pid encodes as an index key"),
            budget: AtomicUsize::new(0),
            armed: AtomicBool::new(false),
            commits: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
        })
    }

    fn store(self: &Arc<Self>) -> Arc<dyn KvStore> {
        self.clone()
    }

    /// Let `budget` more operation commits land, then refuse every one from the next
    /// on. `arm(1)` is "the intent lands, the claim is refused".
    fn arm(&self, budget: usize) {
        self.budget.store(budget, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// The writer works again.
    fn heal(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    /// Operation commits attempted so far.
    fn commits(&self) -> usize {
        self.commits.load(Ordering::SeqCst)
    }

    /// How many commits the cut refused.
    fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }

    /// Whether this write set is an operation commit of the run, and whether the
    /// budget refuses it.
    async fn blocks(&self, ops: &[StoreBatchOp]) -> bool {
        let touches_document = ops.iter().any(|op| match op {
            StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => key == &self.key,
        });
        if !touches_document || !self.changes_recovery_index(ops).await {
            return false;
        }
        self.commits.fetch_add(1, Ordering::SeqCst);
        if !self.armed.load(Ordering::SeqCst) {
            return false;
        }
        let left = self.budget.load(Ordering::SeqCst);
        if left == 0 {
            self.refused.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            self.budget.store(left - 1, Ordering::SeqCst);
            false
        }
    }

    /// Whether this write set changes the run's recovery index — what an operation
    /// commit does and an outbox delivery does not.
    async fn changes_recovery_index(&self, ops: &[StoreBatchOp]) -> bool {
        let stored = self.inner.one(&self.recovery_key).await.ok().flatten();
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } if key == &self.recovery_key => {
                    return stored.as_deref() != Some(value.as_slice());
                }
                StoreBatchOp::Delete { key } if key == &self.recovery_key => {
                    return stored.is_some();
                }
                _ => {}
            }
        }
        false
    }
}

#[async_trait::async_trait]
impl KvStore for DocCutKv {
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

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if self.blocks(ops).await {
            return Err(injected("document write cut"));
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

// ---------------------------------------------------------------------------
// STALL × restart — a cut swept across every write boundary of a completion
// ---------------------------------------------------------------------------

/// The cut is swept across the commit boundaries of a completion: the client
/// completion that drives `act → root` is applied while the store stops
/// honouring commits of the run's aggregate document at exactly the k-th one.
/// Whatever the cut leaves durable — the intent `Pending`, the claim
/// `prepared`, the propagation seeded but undriven — the reloaded engine must
/// carry the run to its end, and the end must be exactly once: one generation
/// per node, one record per operation, and the finished run settled (its rows
/// reclaimed through the sweeper).
///
/// The sweep is over the *completion's own commits* (the legacy cheaper
/// variant): the intent, the claim, the effect (the operation's close included),
/// and — one past the operation — the claim of the propagation it seeds. Those are the
/// boundaries the completion owns; an operation's *row projection* is a derived write
/// whose failure is absorbed, and is pinned by the row-fault cells in `action.rs`
/// and `next.rs`, while the propagation's own effect boundary is the kernel's
/// fail-safe (surfaced for reconciliation), pinned by `next.rs`'s propagation
/// cells.
///
/// What each boundary may leave is bounded: nothing durable is decided by the
/// client's retry (exactly as the refusal promised); a claim the crash stranded
/// before its effect is reclaimed once its lease elapses and surfaced for
/// reconciliation (`effect-unknown`) — never re-applied and never left as a
/// live, drivable decision; and everything else recovery finalizes or re-drives.
/// The cut is at a *commit* boundary, not a raw write, so an interleaved row
/// projection cannot shift it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_cut_swept_across_a_completion_converges_at_every_write_boundary() {
    bounded(
        "a_cut_swept_across_a_completion_converges_at_every_write_boundary",
        cut_sweep_inner(),
    )
    .await;
}

/// The completion operation's own commits: the intent, the claim, and the effect
/// (which closes the operation with it). Cutting the commit *after* them is the
/// propagation's claim.
const COMPLETION_COMMITS: usize = 3;

async fn cut_sweep_inner() {
    for refused_commit in 0..=COMPLETION_COMMITS {
        let key = format!("cutcommit{refused_commit}");
        let kv = DocCutKv::new(&key);
        let store = kv.store();
        let engine = engine_on(store.clone()).await;
        let (pid, tid) = waiting_run_named(&engine, &key).await;

        // Arm the cut *after* the wait is durable, so it refuses the
        // completion's own commit and not a delivery a pass happened to make.
        kv.arm(refused_commit);
        let refused = complete_act(&engine, &pid, &tid).await;
        let fired = poll_until(
            &format!("the cut at commit {refused_commit} to refuse a commit"),
            || async { kv.refused() > 0 },
        )
        .await;
        assert!(
            fired,
            "the cut at commit {refused_commit} never fired: {refused:?}"
        );

        let recovered = crash_and_reboot(&engine, store.clone(), &kv).await;
        // The recovered engine must leave *nothing* of the run undone — every
        // execution terminal, every record delivered, every operation closed or
        // surfaced for reconciliation — before the rows can be swept.
        drive_to_end(&recovered, &pid, &tid).await;
        assert_finished_once(&recovered, &pid, &tid).await;

        // A minute later, recovery must not move the decision either: a claim a
        // cut stranded is reclaimed here into work that is surfaced, never into
        // a second decision.
        let _ = recovered
            .runtime()
            .kernel()
            .recover(SystemClock.now_ms() + 120_000)
            .await;
        assert_still_finished_once(&recovered, &pid, &tid).await;

        sweep_finished_run(&recovered, &pid).await;
        stop(&recovered).await;
    }
}

// ---------------------------------------------------------------------------
// STALL × DB失败 — a stranded operation is re-driven without a restart
// ---------------------------------------------------------------------------

/// A run stranded at its completion's *claim*: the intent is durable and
/// undecided, the act still waits, and the store refuses every later commit of
/// the run until [`DocCutKv::heal`].
///
/// The cut is re-armed on a fresh run until it lands on that boundary. The arm
/// counts *operation commits*, so the only way an attempt can miss is an operation the
/// engine ran between the arm and the action (the store is otherwise quiet);
/// re-arming keeps the cell's fault the one it means to cut instead of letting
/// the assertion below fail on a shifted boundary. The strand it finally cuts
/// is the one the cell asserts — three properties, all checked before the
/// caller heals anything.
async fn stranded_at_claim(pid: &str) -> (Engine, Arc<DocCutKv>, String) {
    for _ in 0..8 {
        let kv = DocCutKv::new(pid);
        let store = kv.store();
        let engine = engine_on(store).await;
        let (pid, tid) = waiting_run_named(&engine, pid).await;
        kv.arm(CLAIM_WRITE - 1);
        let refused = complete_act(&engine, &pid, &tid).await;
        let shapes = open_shapes(&engine, &pid, &tid).await;
        let decided = decided_state(&engine, &pid, &tid).await;
        if refused.is_err()
            && kv.refused() == 1
            && shapes == vec!["pending"]
            && decided == Some(TaskState::Waiting)
        {
            return (engine, kv, pid);
        }
        eprintln!(
            "the claim cut missed (refused={}, operation commits={}, shapes={shapes:?}, decided={decided:?}); re-arming",
            kv.refused(),
            kv.commits()
        );
        stop(&engine).await;
    }
    panic!("the completion's claim could not be cut after eight attempts");
}

/// A fresh engine over the healed store whose *recovery* is cut as well: an operation
/// commit of the run is refused, so the stranded intent is not carried forward
/// and survives the restart.
///
/// The cut is re-armed until it lands on the recovery operation itself — its claim
/// refused (the intent stays `Pending`) or, when a commit the booted engine made
/// first took the refusal, its effect refused (the intent's claim `prepared`).
/// Both leave the work unresolved, which is the strand the cell means; a
/// recovery that actually carried the run forward is a miss and is retried.
async fn boot_with_recovery_cut(store: Arc<dyn KvStore>, kv: &Arc<DocCutKv>, pid: &str) -> Engine {
    for _ in 0..8 {
        kv.heal();
        let engine = engine_on(store.clone()).await;
        quiesce(&engine).await;
        let refused_before = kv.refused();
        kv.arm(0);
        let _ = engine
            .runtime()
            .kernel()
            .recover(SystemClock.now_ms())
            .await;
        let fired = poll_settle(|| async { kv.refused() > refused_before }, 150).await;
        let shapes = open_shapes(&engine, pid, "s1").await;
        if fired && matches!(shapes.as_slice(), ["pending"] | ["prepared"]) {
            return engine;
        }
        eprintln!("the recovery cut missed (fired={fired}, shapes={shapes:?}); re-arming");
        stop(&engine).await;
    }
    panic!("the recovery of {pid} could not be cut at its own operation after eight attempts");
}

/// A durable intent a crash left `Pending` — the completion's own claim write
/// refused, so the decision is recorded but nothing owns it — is re-driven by
/// the engine's own recovery, with **no restart and no client action**: the
/// healed store lets the scan wake the intent, the operation decides the act exactly
/// once, the chain propagates into the root, the run settles and its rows are
/// reclaimed (see [`sweep_finished_run`] for the one delivery-lifecycle hole
/// that can keep them).
///
/// Legacy called the re-driver the "outbox pass", and it re-dispatched an
/// abandoned `next` record. The kernel keeps that record inside the aggregate,
/// so the stranded work is an *operation* left non-terminal and the pass that
/// re-derives it is the recovery scan (`KernelHost::recover`, which the driver
/// tick runs too). The cell proves the strand first — the intent is durable and
/// `pending`, the act still waits — so it cannot pass vacuously.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_operation_is_redriven_by_the_recovery_pass() {
    bounded(
        "a_stalled_operation_is_redriven_by_the_recovery_pass",
        stalled_redriven_inner(),
    )
    .await;
}

async fn stalled_redriven_inner() {
    let (engine, kv, pid) = stranded_at_claim("strandedpass").await;
    let tid = "s1".to_string();

    // The store comes back; the engine keeps running and no client acts again.
    // One round of the engine's own pass — the same scan the driver tick runs —
    // finds the stranded intent; the driver carries the wakes it hands out from
    // there, and the cell only reads.
    kv.heal();
    let scan = {
        let runtime = engine.runtime();
        let kernel = runtime.kernel();
        kernel
            .recover(SystemClock.now_ms())
            .await
            .unwrap_or_default()
    };
    assert!(
        scan.woken.iter().any(|op| op.process_id == pid),
        "the pass must re-derive the stranded intent: {scan:?}"
    );
    // The driver carries the wakes from there; the cell only reads — except for
    // the retry a *transient* needs (see `drive_to_end`): a re-driven operation whose
    // own commit lost a race is reclaimed and delivered back to the client,
    // whose decision the kernel's guard admits exactly once.
    drive_to_end(&engine, &pid, &tid).await;

    assert_finished_once(&engine, &pid, &tid).await;
    for shape in open_shapes(&engine, &pid, &tid).await {
        assert_eq!(
            shape, "effect-unknown",
            "a re-driven operation closes, or is surfaced for reconciliation"
        );
    }
    sweep_finished_run(&engine, &pid).await;
    stop(&engine).await;
}

// ---------------------------------------------------------------------------
// STALL × restart — the same strand, re-driven by the periodic tick alone
// ---------------------------------------------------------------------------

/// The same stranded intent, healed and left to the engine's own timer: the
/// tick's recovery scan re-drives it, so an abandoned operation is not a
/// restart-shaped hole in the runtime's contract. The cell waits read-only for
/// the tick to converge the run; only if a re-driven operation's own commit lost a
/// race and the run is still stalled does it fall back to the retry
/// [`drive_to_end`] applies — printing a NOTE, since that path is not the
/// timer's.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_operation_is_redriven_by_the_timer() {
    bounded(
        "a_stalled_operation_is_redriven_by_the_timer",
        stalled_timer_inner(),
    )
    .await;
}

async fn stalled_timer_inner() {
    let (engine, kv, pid) = stranded_at_claim("strandedtimer").await;
    let tid = "s1".to_string();

    // Heal and wait: the only driver left is the periodic tick, which re-drives
    // the stranded intent on its own.
    kv.heal();
    if !poll_settle(|| async { settle_run(&engine, &pid).await }, 250).await {
        eprintln!(
            "NOTE: the tick did not converge {pid}; falling back to the retry a transient needs"
        );
        drive_to_end(&engine, &pid, &tid).await;
    }

    assert_finished_once(&engine, &pid, &tid).await;
    for shape in open_shapes(&engine, &pid, &tid).await {
        assert_eq!(
            shape, "effect-unknown",
            "a timer re-drive closes the operation, or surfaces it for reconciliation"
        );
    }
    sweep_finished_run(&engine, &pid).await;
    stop(&engine).await;
}

// ---------------------------------------------------------------------------
// STALL × restart — a strand that survives a second restart
// ---------------------------------------------------------------------------

/// Two restarts over the same stranded intent: the crash that leaves the
/// completion's intent `Pending`, a second engine whose own recovery is cut as
/// well (its first write is refused, so the strand is not carried forward), and
/// a third engine over the whole store that must finish the run — with the
/// re-driven decision landing exactly once and every row of it reclaimed.
///
/// The legacy cell cut the second engine's recovery too; the kernel's strand is
/// the same durable operation, and the property is that a recovery which fails
/// leaves the *intent* — never a half-applied decision — so the next engine
/// re-drives the same work rather than replaying a decision it cannot prove was
/// made.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_operation_survives_a_second_restart() {
    bounded(
        "a_stalled_operation_survives_a_second_restart",
        second_restart_inner(),
    )
    .await;
}

async fn second_restart_inner() {
    let (engine_a, kv, pid) = stranded_at_claim("strandedtwice").await;
    let tid = "s1".to_string();
    let store = kv.store();

    // The first crash: the completion's claim commit is refused, leaving the
    // intent durable and undecided, and the engine dies without recovering it.
    stop(&engine_a).await;

    // The second engine: its own recovery is cut too — the very next operation commit
    // of the run (the recovery operation's claim) is refused — so the strand survives
    // this restart.
    let engine_b = boot_with_recovery_cut(store.clone(), &kv, &pid).await;
    stop(&engine_b).await;

    // The third engine over the whole store carries the run to its end.
    kv.heal();
    let engine_c = engine_on(store.clone()).await;
    drive_to_end(&engine_c, &pid, &tid).await;
    assert_finished_once(&engine_c, &pid, &tid).await;
    // The finish closes the stranded intent; an operation engine B's cut left
    // `prepared` is reclaimed by the third engine's recovery into the one
    // surface the kernel keeps for an effect boundary — surfaced for
    // reconciliation — never a second decision.
    for shape in open_shapes(&engine_c, &pid, &tid).await {
        assert_eq!(
            shape, "effect-unknown",
            "the finish may leave only work surfaced for reconciliation"
        );
    }
    sweep_finished_run(&engine_c, &pid).await;
    stop(&engine_c).await;
}

// ---------------------------------------------------------------------------
// STALL × boot — a run parked at boot waits for a slot and converges
// ---------------------------------------------------------------------------

/// The boot pass must never grow a run the slot bound does not hold. The
/// kernel's counterpart of the legacy boot replay is slot admission: with
/// `cache.cap = 1` a start beyond the cap *parks* — its root stays `Created`
/// and its seeded operation stays `Pending` (the read model's `None` process
/// state, the legacy parked row) — and a fresh engine's recovery scan is what
/// places the runs already in flight, one slot at a time.
///
/// With one slot, a run that reached its act and three runs parked behind it
/// are all in the store at boot. The scan admits exactly one of the parked
/// runs — the others take no operation at all, their aggregate staying exactly the
/// seed the crash left — and as each admitted run is decided by the client and
/// settles, the slot it frees admits the run that has waited longest
/// (`__created` ascending): the second and third seatings are asserted in that
/// order. The run waiting on the client takes no operation and no slot, and
/// converges when its own act is decided.
///
/// The kernel admits the *first* `cap` runs at boot in the order the scan
/// returns them, not by `__created` (`Admissions` says so: the store keeps no
/// boot index), so this cell asserts the admission it observes and the FIFO
/// only where the kernel promises it — at a freed slot. Legacy deferred the
/// replayed record of a process the resident set did not hold; the kernel holds
/// no resident object, so what it defers is the *operation*.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_run_parked_at_boot_waits_for_a_slot_and_converges() {
    bounded(
        "a_run_parked_at_boot_waits_for_a_slot_and_converges",
        boot_slot_inner(),
    )
    .await;
}

async fn boot_slot_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let mut config = Config::default();
    config.data.cache_cap = Some(1);

    let engine = engine_with(store.clone(), &config).await;
    // The head run is admitted and reaches its waiting act: it holds the slot.
    let (head, head_tid) = waiting_run(&engine).await;
    // Three more starts find the slot taken and park, oldest first: the order a
    // freed slot must admit them in.
    let mut parked = Vec::new();
    for index in 0..3 {
        tokio::time::sleep(Duration::from_millis(2)).await;
        let pid = format!("parked{index}");
        let started = start_run_with(
            &engine,
            &one_step_irq(&format!("act{index}")),
            Vars::new().with(utils::consts::PROCESS_ID, &pid),
        )
        .await;
        assert_eq!(started, pid, "the run must take the pid the cell named");
        parked.push(started);
    }
    for pid in &parked {
        assert_parked(&engine, pid).await;
        assert!(
            !completed(&engine, pid).await,
            "a parked run has not been driven, let alone settled"
        );
    }
    stop(&engine).await;

    // Boot over the same store under the same cap. The head run holds only the
    // client's work (a waiting act carries no operation), so the single slot
    // contests the three parked runs' seeded root operations alone.
    let engine2 = engine_with(store.clone(), &config).await;

    // Exactly one parked run is admitted into its act; the others do not operation at
    // all — their aggregate is still the seed the crash left.
    let before = snapshot_map(&engine2, &parked).await;
    let first = wait_admitted(&engine2, &parked, &before).await;
    let mut waiting: Vec<String> = parked
        .iter()
        .filter(|pid| **pid != first)
        .cloned()
        .collect();

    // Decide the admitted run's act: it settles and frees the slot, which then
    // admits the parked run that has waited longest — `__created` ascending.
    complete_act(&engine2, &first, "s1").await.unwrap();
    wait_completed(&engine2, &first).await;
    let before = snapshot_map(&engine2, &waiting).await;
    let second = wait_admitted(&engine2, &waiting, &before).await;
    assert_eq!(
        second, waiting[0],
        "a freed slot admits the run that has waited longest"
    );
    waiting.retain(|pid| *pid != second);
    complete_act(&engine2, &second, "s1").await.unwrap();
    wait_completed(&engine2, &second).await;

    // The last parked run is admitted, and the head run is decided by its own
    // client; every run keeps one generation per node and one record per operation.
    let before = snapshot_map(&engine2, &waiting).await;
    let third = wait_admitted(&engine2, &waiting, &before).await;
    assert_eq!(third, waiting[0], "the last parked run takes the last seat");
    complete_act(&engine2, &third, "s1").await.unwrap();
    wait_completed(&engine2, &third).await;
    complete_act(&engine2, &head, &head_tid).await.unwrap();
    wait_completed(&engine2, &head).await;

    for pid in parked.iter().chain(std::iter::once(&head)) {
        assert_finished_once(&engine2, pid, "s1").await;
        sweep_finished_run(&engine2, pid).await;
    }
    stop(&engine2).await;
}

/// The aggregates of `pids`, for a cell to prove which run a boot pass moved.
async fn snapshot_map(engine: &Engine, pids: &[String]) -> BTreeMap<String, ProcessSnapshot> {
    let mut out = BTreeMap::new();
    for pid in pids {
        out.insert(pid.clone(), process_snapshot(engine, pid).await);
    }
    out
}

/// Poll until one of `pids` has been admitted into its act, and answer which.
/// Every other run must still hold exactly its pre-admission aggregate: a run
/// the single slot did not admit takes no operation at all.
async fn wait_admitted(
    engine: &Engine,
    pids: &[String],
    before: &BTreeMap<String, ProcessSnapshot>,
) -> String {
    let what = format!("one of {pids:?} to be admitted");
    poll_until_dumped(
        &what,
        || async {
            for pid in pids {
                if proc_row(engine, pid)
                    .await
                    .is_some_and(|proc| waiting_task(&proc, "s1").is_some())
                {
                    return true;
                }
            }
            false
        },
        || outstanding(engine, &pids[0]),
    )
    .await;

    let mut admitted = None;
    for pid in pids {
        let waiting = proc_row(engine, pid)
            .await
            .is_some_and(|proc| waiting_task(&proc, "s1").is_some());
        if waiting {
            assert!(admitted.is_none(), "only one slot exists");
            admitted = Some(pid.clone());
        }
    }
    let admitted = admitted.expect("one run must be admitted");
    for pid in pids.iter().filter(|pid| **pid != admitted) {
        assert_eq!(
            &process_snapshot(engine, pid).await,
            before.get(pid).expect("a snapshot per run"),
            "a run the slot did not admit may not operation"
        );
    }
    admitted
}

/// The parked shape of one run, on the aggregate and on the row: the root stays
/// `Created`, its single seeded operation is still `Pending` (no operation), no
/// message was written, and the read model spells the process state `None` —
/// the legacy parked row.
async fn assert_parked(engine: &Engine, pid: &str) {
    let snapshot = process_snapshot(engine, pid).await;
    assert!(snapshot.outbox.is_empty(), "a parked run writes no message");
    assert!(
        snapshot
            .tasks
            .values()
            .all(|task| task.state == TaskState::Created),
        "a parked run's root stays `Created`"
    );
    assert!(
        snapshot
            .operations
            .values()
            .all(|operation| matches!(operation.state, OperationState::Pending)),
        "the seeded operation waits for a slot"
    );
    // the row is projected through the write-behind queue
    flush(engine).await;
    let row = proc_row(engine, pid)
        .await
        .expect("a parked run keeps its row");
    assert_eq!(
        row.state(),
        LegacyTaskState::None,
        "the parked process state, as the legacy row spells it"
    );
}
