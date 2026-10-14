//! The shell: the async half of the runtime, driving the pure `core` over the
//! `durable` byte layer.
//!
//! [`Runtime`](crate::runtime::Runtime) above owns it — it is the entry point
//! and the composition root; this module is what that entry point *runs*.
//!
//! The shell owns no state of its own — every fact lives in the aggregate
//! documents over the engine's [`KvStore`]. Wakes are a channel and strictly an
//! optimization: losing one is safe because `recover` re-derives the same
//! work from the recovery index. It drives one operation kind per durable hop:
//!
//! - `ExecuteTask` commits `Ready → Running` (the task started) and seeds the
//!   `ApplyAction` operation whose effect performs the task's work: the
//!   attached [`effects::EffectRunner`] runs the step's package (the serving
//!   shell has [`PackageEffectRunner`]; a shell without one auto-completes).
//! - `ApplyAction` commits `Running → Completed` (the effect is durable) and
//!   seeds the `PropagateTask` operation in the same atomic batch — a crash
//!   between effect and propagation leaves the operation open, never the
//!   reverse.
//! - `PropagateTask` re-plans propagation from the authoritative snapshot and
//!   commits the successors' `Created → Ready` plus their `ExecuteTask`
//!   operations; join readiness omits waiting successors idempotently.
//!
//! Every hop is one `commit_transition` with the hop's own operation as the
//! `source_operation`, so the operation is committed inside the same atomic
//! batch as its business effect. When a hop's durable remainder already
//! exists (a wake redelivered after the commit), the hop commits an empty
//! plan sourced by its own operation, which completes the operation without
//! touching business state.

use crate::runtime::core::model::clock::Clock;
use crate::runtime::core::model::clock::SystemClock;
use crate::runtime::core::model::operation::OperationEvent;
use crate::runtime::core::model::operation::OperationId;
use crate::runtime::core::model::operation::OperationKind;
use crate::runtime::core::model::operation::OperationState;
use crate::runtime::core::model::transition_plan::OperationCommit;
use crate::runtime::core::model::transition_plan::OperationIntent;
use crate::runtime::core::model::transition_plan::StateChange;
use crate::runtime::core::model::transition_plan::TransitionPlan;
use crate::runtime::core::model::transition_plan::VariableWrite;
use crate::runtime::core::outbox::records::DeliveryClaim;
use crate::runtime::core::outbox::records::OutboxEvent;
use crate::runtime::core::plan::branch::BranchCandidate;
use crate::runtime::core::plan::branch::BranchCommand;
use crate::runtime::core::plan::branch::BranchDecision;
use crate::runtime::core::plan::branch::BranchPlanner;
use crate::runtime::core::plan::branch::PredicateBasis;
use crate::runtime::core::plan::branch::PredicateValue;
use crate::runtime::core::plan::concurrency;
use crate::runtime::core::plan::concurrency::Admission;
use crate::runtime::core::plan::container::ContainerChild;
use crate::runtime::core::plan::container::ContainerChildPlan;
use crate::runtime::core::plan::container::ContainerFanoutCommand;
use crate::runtime::core::plan::container::ContainerPredecessor;
use crate::runtime::core::plan::propagation::PropagateCommand;
use crate::runtime::core::plan::propagation::PropagationPlanner;
use crate::runtime::core::recovery::planner::RecoveryDisposition;
use crate::runtime::core::recovery::planner::RecoveryPlanner;
use crate::runtime::core::recovery::planner::RecoveryReport;
use crate::runtime::durable::async_document_store::AsyncAggregateRepository;
use crate::runtime::durable::async_document_store::AsyncRecoveryRepository;
use crate::runtime::durable::async_document_store::KvDocumentStore;
use crate::store::KvStore;
use crate::{ActError, Result};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender, channel};

// The shell's parts are its own submodules: they see this module's scope and
// its private items, and this module re-exports the ones the runtime names
// through it.
mod action;
mod effects;
mod errors;
mod facts;
mod message;
mod model;
mod planning;
mod predicate;
mod projection;
mod scope;

use self::model::execution_graph;
use self::{action::*, errors::*, facts::*, planning::*};
pub(crate) use effects::PackageEffectRunner;
pub(crate) use message::MessageForwarder;
pub(crate) use model::ProcessSpec;
pub(crate) use predicate::EnginePredicateRunner;
pub(crate) use projection::RowProjector;

const OWNER: &str = "kernel-host";
const LEASE_MS: i64 = 60_000;
/// How many times a client action re-plans after losing a compare-and-swap:
/// the serving loop commits hops of the same aggregate concurrently, so a
/// client's plan can go stale before its commit. A retry re-reads the
/// snapshot; the action then either applies or is legitimately refused.
const ACTION_ATTEMPTS: usize = 8;
const DELIVERY_LEASE_MS: i64 = 30_000;
const PAGE_SIZE: usize = 128;

/// Best-effort wake delivery into the host's queue. Losing a wake is safe:
/// the recovery scan re-derives the same pending operations.
#[derive(Clone)]
struct QueueWake {
    tx: Sender<OperationId>,
}

impl QueueWake {
    fn wake(&self, operation: &OperationId) {
        // A full or closed queue loses the wake by design; the recovery scan
        // is the durable net.
        let _ = self.tx.try_send(operation.clone());
    }
}

/// What one quiescence pass did.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunReport {
    pub(crate) executed: usize,
    pub(crate) retried: usize,
    pub(crate) ignored: usize,
}

/// The engine-facing kernel host: a cloneable handle over the aggregate
/// documents. The wake channel's receiving end is returned by [`KernelHost::new`]
/// and belongs to exactly one driver task ([`KernelHost::serve`]).
#[derive(Clone)]
pub(crate) struct KernelHost {
    aggregates: Arc<AsyncAggregateRepository>,
    recovery: Arc<AsyncRecoveryRepository>,
    wake_tx: Sender<OperationId>,
    /// The effect seam: when attached, a Func package's work runs inside the
    /// ApplyAction hop and its outputs commit with the completion. Without
    /// one the host auto-completes (the shadow contract).
    effect: Arc<parking_lot::RwLock<Option<Arc<dyn effects::EffectRunner>>>>,
    /// The outbox delivery seam: the shadow logs; the driver's transport
    /// maps records onto engine messages (see `message::message_from_record`).
    transport: Arc<
        parking_lot::RwLock<
            Box<dyn crate::runtime::core::outbox::outbox_dispatch::OutboxTransport>,
        >,
    >,
    /// The process-lifecycle seam: when attached, a run's start and terminal
    /// are reported as engine events — the driver's mirror of the legacy
    /// `on_proc` emission. The host builds the `Message` from the aggregate,
    /// its only source.
    lifecycle: Arc<parking_lot::RwLock<Option<LifecycleSink>>>,
    /// The read-model seam: when attached, every commit re-materializes the
    /// legacy rows (`procs`/`tasks`/`vars`) from the committed snapshot, so
    /// the row-reading APIs and the sweeper see the run (see `projection`).
    /// Without one the host keeps no read model — the kernel's own tests.
    projection: Arc<parking_lot::RwLock<Option<Arc<projection::RowProjector>>>>,
    /// The predicate seam: when attached, a branch `if` is decided on the
    /// engine's own environment (`ctx.eval`, engine functions included) on a
    /// headless task, exactly like the legacy branch task; without one the
    /// built-in subset evaluator decides (see `predicate`).
    predicate: Arc<parking_lot::RwLock<Option<Arc<dyn predicate::PredicateRunner>>>>,
    /// Which runs were already reported: a start once, a terminal once.
    reported: Arc<parking_lot::Mutex<ReportedRuns>>,
    /// The slot policy (`crate::runtime::core::plan::concurrency`): the runs
    /// in flight at once, and the parked ones waiting for a slot. `start_process`
    /// reserves here and the driver settles here, so one lock owns the answer
    /// to "may this run hop".
    admissions: Arc<parking_lot::Mutex<Admissions>>,
}

/// Which engine event a run's lifecycle report is: the driver's mirror of the
/// legacy `on_proc` emission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessEvent {
    Started,
    Completed,
    Errored,
}

/// The process-lifecycle sink: the engine routes it onto its channel events
/// (`on_start`/`on_complete`/`on_error`).
pub(crate) type LifecycleSink = Box<dyn Fn(&crate::Message, ProcessEvent) + Send + Sync>;

/// Which runs were already reported: a start once, a terminal once.
#[derive(Default)]
struct ReportedRuns {
    started: std::collections::HashSet<String>,
    finished: std::collections::HashSet<String>,
}

/// The driver's slot bookkeeping: which runs hold one of the `cache_cap`
/// slots, and which wait for one.
///
/// The configured number used to bound the legacy engine's *resident object
/// set*; the kernel holds no resident objects, so the same number is redefined
/// as the bound on runs that are in flight at once (see
/// [`concurrency`](crate::runtime::core::plan::concurrency) for the rule).
/// A run that cannot take a slot *parks*: a start seeds it with its root still
/// `Created` — the read model's `None` process state, exactly the legacy parked
/// row — and leaves that root's operation undriven, while a run that was
/// already in flight keeps the row it had (the legacy boot-resume overflow's
/// shape). Either way the run takes no hop, and the oldest parked run
/// (`__created` ascending, the legacy row's `timestamp` order) is what a freed
/// slot admits.
///
/// The legacy resident set answered the same question through `Cache`; here
/// the only two writers are [`KernelHost::start_process`] (which reserves) and
/// the driver's hops (which settle), so this set is the single answer to "may
/// this run hop". Like that set it is in-memory: a restart starts it empty,
/// and the runs still in flight take their slots again through
/// [`KernelHost::place_run`] when the recovery scan hands them their first
/// wake. The first `cap` of them are admitted in the order the scan returns
/// them rather than by `__created` (the store keeps no boot index to sort by),
/// while every later admission is by stamp, exactly like the legacy parked
/// queue.
#[derive(Default)]
struct Admissions {
    /// The bound on runs in flight at once.
    cap: usize,
    /// Runs holding a slot: admitted here and not yet settled.
    active: std::collections::HashSet<String>,
    /// Parked runs, oldest first, with the operation that admits each: the
    /// seeded root `ExecuteTask` whose hop starts the parked root. The key is
    /// the run's `__created` stamp; two runs seeded in the same millisecond
    /// are ordered by pid, so no run can hide behind an equal key.
    parked: std::collections::BTreeMap<(i64, String), OperationId>,
}

impl Admissions {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            ..Self::default()
        }
    }

    /// The pure rule against the set it bounds.
    fn decide(
        &self,
        state: crate::runtime::core::model::execution_graph::ExecutionState,
    ) -> Admission {
        concurrency::admit(self.cap, self.active.len(), state)
    }

    /// Reserve a slot for a fresh run — its root is seeded `Ready`, which is
    /// the state the rule is asked about — or decide that it parks. The
    /// reservation and the decision are one step, so concurrent starts cannot
    /// both take the last slot.
    fn reserve(&mut self, process_id: &str) -> Admission {
        let admission =
            self.decide(crate::runtime::core::model::execution_graph::ExecutionState::Ready);
        if admission == Admission::Admit {
            self.activate(process_id);
        }
        admission
    }

    fn is_parked(&self, process_id: &str) -> bool {
        self.parked.keys().any(|(_, parked)| parked == process_id)
    }

    /// Whether the run holds a slot.
    fn holds(&self, process_id: &str) -> bool {
        self.active.contains(process_id)
    }

    fn activate(&mut self, process_id: &str) {
        self.active.insert(process_id.to_string());
    }

    fn park(&mut self, created_ms: i64, process_id: &str, admission: &OperationId) {
        self.parked
            .insert((created_ms, process_id.to_string()), admission.clone());
    }

    /// Forget a reservation whose run never became durable (its creation
    /// failed): the slot must not stay taken — and the pid must not stay
    /// parked — by a run that does not exist.
    fn abandon(&mut self, process_id: &str) {
        self.active.remove(process_id);
        self.parked.retain(|(_, parked), _| parked != process_id);
    }

    /// Take a free slot for a parked run — the check a start makes after it
    /// parked, because a slot can free while its aggregate is being written
    /// and nothing else would come back for that run.
    fn take_free_slot(&mut self, created_ms: i64, process_id: &str) -> bool {
        if self.active.len() >= self.cap
            || self
                .parked
                .remove(&(created_ms, process_id.to_string()))
                .is_none()
        {
            return false;
        }
        self.activate(process_id);
        true
    }

    /// A settled run frees its slot; returns the operation that admits the
    /// parked run taking it, if there is one.
    fn release(&mut self, process_id: &str) -> Option<OperationId> {
        if !self.active.remove(process_id) {
            return None;
        }
        if self.active.len() >= self.cap {
            return None;
        }
        let ((_, process_id), admission) = self.parked.pop_first()?;
        self.active.insert(process_id);
        Some(admission)
    }
}

/// The operation that admits a run the driver did not seed itself — one
/// already in flight when it booted: the seed's root `ExecuteTask` (sequence 0
/// of the root execution), which is the hop that starts a `Created` root. A run
/// whose root hop already ran has no such operation left, so waking it is a
/// no-op and the recovery scan re-wakes the run's real work on the next tick.
fn resume_operation(
    snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
) -> OperationId {
    let root = root_execution(snapshot);
    OperationId {
        process_id: snapshot.process_id.clone(),
        task_id: root
            .as_ref()
            .map(|root| root.task_id.clone())
            .unwrap_or_else(|| crate::utils::consts::TASK_ROOT_TID.to_string()),
        generation: root.map(|root| root.generation).unwrap_or(0),
        sequence: 0,
    }
}

/// The default outbox transport: it logs each record and confirms it — what a
/// host with nothing attached (the kernel's own tests, and any embedder that
/// only wants the durable aggregate) does. The serving host replaces it with
/// [`message::MessageForwarder`] so records reach the engine channel.
struct ShadowTransport;

impl crate::runtime::core::outbox::outbox_dispatch::OutboxTransport for ShadowTransport {
    fn deliver(
        &mut self,
        record: &crate::runtime::core::outbox::records::OutboxRecord,
    ) -> crate::runtime::core::outbox::outbox_dispatch::DeliveryOutcome {
        tracing::debug!(
            pid = %record.id.process_id,
            key = %record.id.key,
            topic = %record.topic,
            "kernel outbox delivered (shadow)"
        );
        crate::runtime::core::outbox::outbox_dispatch::DeliveryOutcome::Delivered {
            receipt: "shadow".into(),
        }
    }
}

impl KernelHost {
    /// A host over the engine's key-value store, plus the wake receiver its
    /// driver task must own. `cache_cap` bounds the runs that may be in flight
    /// at once (see [`Admissions`]); tests pass
    /// `Arc::new(MemoryStore::new())` and a cap.
    pub(crate) fn new(
        kv: Arc<dyn KvStore>,
        cache_cap: usize,
    ) -> (Arc<Self>, Receiver<OperationId>) {
        let store = Arc::new(KvDocumentStore::new(kv));
        let aggregates = Arc::new(AsyncAggregateRepository::new(store));
        let recovery = Arc::new(AsyncRecoveryRepository::new(aggregates.clone()));
        let (wake_tx, wake_rx) = channel(PAGE_SIZE * 8);
        (
            Arc::new(Self {
                aggregates,
                recovery,
                wake_tx,
                effect: Arc::new(parking_lot::RwLock::new(None)),
                transport: Arc::new(parking_lot::RwLock::new(Box::new(ShadowTransport))),
                lifecycle: Arc::new(parking_lot::RwLock::new(None)),
                projection: Arc::new(parking_lot::RwLock::new(None)),
                predicate: Arc::new(parking_lot::RwLock::new(None)),
                reported: Arc::new(parking_lot::Mutex::new(ReportedRuns::default())),
                admissions: Arc::new(parking_lot::Mutex::new(Admissions::new(cache_cap))),
            }),
            wake_rx,
        )
    }

    /// A host over an in-memory store for tests and the P2 parity harness,
    /// with no slot bound: the kernel's own tests start runs they intend to
    /// drive, and the parking behaviour has its own tests with a cap.
    #[cfg(test)]
    pub(crate) fn in_memory() -> (Arc<Self>, Receiver<OperationId>) {
        Self::in_memory_with_cap(usize::MAX)
    }

    /// An in-memory host with an explicit slot bound — the tests that park.
    #[cfg(test)]
    pub(crate) fn in_memory_with_cap(cache_cap: usize) -> (Arc<Self>, Receiver<OperationId>) {
        Self::new(Arc::new(crate::store::MemoryStore::new()), cache_cap)
    }

    /// Attach the effect runner: from then on a Func package's work runs
    /// inside the ApplyAction hop (see [`effects`]). Idempotent — the last
    /// attach wins; attach before driving hops.
    pub(crate) fn attach_effect_runner(&self, runner: Arc<dyn effects::EffectRunner>) {
        *self.effect.write() = Some(runner);
    }

    /// Replace the outbox transport: the shadow's logs records; the driver's
    /// transport maps them onto engine messages (see [`message`]). Attach
    /// before driving hops.
    pub(crate) fn attach_outbox_transport(
        &self,
        transport: Box<dyn crate::runtime::core::outbox::outbox_dispatch::OutboxTransport>,
    ) {
        *self.transport.write() = transport;
    }

    /// Attach the process-lifecycle sink: from then on a run reports one
    /// start when it is alive and one terminal event when every execution
    /// settled — the driver's `on_proc`. Attach before driving hops.
    pub(crate) fn attach_lifecycle_sink(&self, sink: LifecycleSink) {
        *self.lifecycle.write() = Some(sink);
    }

    /// Attach the read-model projector: from then on every commit
    /// re-materializes the legacy rows from the committed snapshot (see
    /// [`projection`]). Attach before driving hops.
    pub(crate) fn attach_row_projector(&self, projector: Arc<projection::RowProjector>) {
        *self.projection.write() = Some(projector);
    }

    /// Attach the predicate runner: from then on a branch `if` is decided on
    /// the engine's own environment on a headless task (see [`predicate`]);
    /// without one the built-in subset evaluator decides. Attach before
    /// driving hops.
    pub(crate) fn attach_predicate_runner(&self, runner: Arc<dyn predicate::PredicateRunner>) {
        *self.predicate.write() = Some(runner);
    }

    /// Decide one branch `if`: on the engine's environment when a predicate
    /// runner is attached — a compile/eval failure of a *usable* expression
    /// included — otherwise on the built-in subset evaluator, which knows
    /// variables, operators and value methods but no engine function.
    async fn evaluate_branch_predicate(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        expr: &str,
        inputs: &[(String, String)],
    ) -> Result<PredicateValue> {
        let runner = self.predicate.read().clone();
        let (Some(runner), Some(model)) = (runner, model_of(snapshot)?) else {
            return Ok(evaluate_predicate(snapshot, execution_id, expr, inputs));
        };
        match runner
            .evaluate(predicate::PredicateRequest {
                scope: self.scope_spec(snapshot, execution_id, model)?,
                expr: expr.to_string(),
                overlay: inputs.to_vec(),
            })
            .await
        {
            Ok(value) => Ok(value),
            // A predicate the engine's environment cannot decide — an unset
            // variable, a non-boolean, a compile error — is the legacy branch
            // task's `init` error: the branch fails, the hop does not.
            Err(error) => Ok(PredicateValue::Invalid {
                message: format!("{error}"),
            }),
        }
    }

    /// Project one committed snapshot into the legacy rows, when a projector
    /// is attached. The rows are a derived read model: a store failure is
    /// reported and dropped, never propagated into the hop — the next commit
    /// re-derives them from the aggregate.
    async fn project(&self, snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot) {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return;
        };
        if let Err(error) = projector.project(snapshot).await {
            tracing::warn!(
                pid = %snapshot.process_id,
                error = %error,
                "row projection failed; the next commit re-derives the rows"
            );
        }
    }

    /// Project only the executions one commit touched (plus the proc row): the
    /// read model's rows for the rest of the run cannot have changed.
    async fn project_touched(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        touched: &std::collections::BTreeSet<String>,
    ) {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return;
        };
        if let Err(error) = projector.project_nodes(snapshot, Some(touched)).await {
            tracing::warn!(
                pid = %snapshot.process_id,
                error = %error,
                "row projection failed; the next commit re-derives the rows"
            );
        }
    }

    /// Re-project one process from its durable aggregate (the action path, and
    /// the outbox settling pass: a run whose last hop committed before its
    /// records were delivered only becomes `removable` once they are).
    pub(crate) async fn refresh_projection(&self, process_id: &str) {
        if self.projection.read().is_none() {
            return;
        }
        match self.load(process_id).await {
            Ok(Some(loaded)) => self.project(&loaded.snapshot).await,
            Ok(None) => {}
            Err(error) => tracing::warn!(
                pid = %process_id,
                error = %error,
                "row projection refresh failed; the next commit re-derives the rows"
            ),
        }
    }

    /// Re-project one process's row only: the deliveries just settled, so the
    /// sole fact that can have changed is the sweeper's `removable` flag.
    async fn refresh_proc_row(&self, process_id: &str) {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return;
        };
        match self.load(process_id).await {
            Ok(Some(loaded)) => {
                if let Err(error) = projector.project_proc_row(&loaded.snapshot).await {
                    tracing::warn!(
                        pid = %process_id,
                        error = %error,
                        "proc row projection failed; the next commit re-derives it"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(
                pid = %process_id,
                error = %error,
                "proc row projection refresh failed"
            ),
        }
    }

    /// Report one run's lifecycle from a fresh snapshot, at most once per
    /// phase. Mirrors the legacy `on_proc`: a start while the run is alive
    /// (running or pending); otherwise — every execution terminal — an error
    /// for a failed root, a completion for any other terminal, with the
    /// model's `exposes` validated against the run's outputs (a validation
    /// failure is reported as an error, exactly like the legacy handler).
    fn report_lifecycle(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
    ) {
        let lifecycle = self.lifecycle.read();
        let Some(sink) = lifecycle.as_ref() else {
            return;
        };
        let Some(root) = snapshot
            .executions
            .keys()
            .filter(|id| id.task_id == crate::utils::consts::TASK_ROOT_TID)
            .min_by_key(|id| id.generation)
            .cloned()
        else {
            return;
        };
        let root_state = snapshot
            .executions
            .get(&root)
            .map(|execution| execution.state);
        let all_terminal = !snapshot.executions.is_empty()
            && snapshot
                .executions
                .values()
                .all(|execution| execution.state.is_terminal());
        let mut reported = self.reported.lock();
        if all_terminal {
            if !reported.finished.insert(snapshot.process_id.clone()) {
                return;
            }
            let mut message = process_message(snapshot, &root);
            if root_state
                == Some(crate::runtime::core::model::execution_graph::ExecutionState::Failed)
            {
                let (code, text) = root_error(snapshot, &root);
                message.set_err(&code, &text);
                sink(&message, ProcessEvent::Errored);
                return;
            }
            match exposes_error(snapshot, &message) {
                Some(error) => {
                    message.set_err("", &error);
                    sink(&message, ProcessEvent::Errored);
                }
                None => sink(&message, ProcessEvent::Completed),
            }
            return;
        }
        if reported.started.insert(snapshot.process_id.clone()) {
            let message = process_message(snapshot, &root);
            sink(&message, ProcessEvent::Started);
        }
    }

    /// The background driver: executes woken hops as they arrive, and on every
    /// tick re-derives recovery work (the durable net) and runs one outbox
    /// delivery pass. Exits when `shutdown` fires.
    pub(crate) async fn serve(
        self: Arc<Self>,
        mut wake_rx: Receiver<OperationId>,
        shutdown: tokio_util::sync::CancellationToken,
        tick: std::time::Duration,
    ) {
        let mut ticker = tokio::time::interval(tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let now = SystemClock.now_ms();
                    if let Err(error) = self.recover(now).await {
                        tracing::warn!(error = %error, "kernel recovery scan failed");
                    }
                    if let Err(error) = self.dispatch_outbox().await {
                        tracing::warn!(error = %error, "kernel outbox pass failed");
                    }
                }
                id = wake_rx.recv() => {
                    let Some(id) = id else { break };
                    // A panic anywhere in a hop must not take the driver down
                    // with it: the engine would be left running with no owner
                    // for any operation. The hop's own user-code boundary (a
                    // package's `execute`) has already turned a package panic
                    // into an ordinary failure, so what reaches here is an
                    // engine panic — reported, and the operation is left open
                    // for the next recovery tick.
                    let run = match CatchPanic::new(self.execute_one(&id)).await {
                        Ok(run) => run,
                        Err(payload) => {
                            let message = panic_message(&payload);
                            tracing::error!(operation = ?id, panic = %message, "kernel hop panicked; left open for recovery");
                            continue;
                        }
                    };
                    if let Err(error) = run {
                        // The operation stays open; the next recovery tick
                        // re-derives it.
                        #[cfg(test)]
                        eprintln!("hop {id:?} failed: {error}");
                        tracing::debug!(operation = ?id, error = %error, "kernel hop failed; left open for recovery");
                    }
                }
            }
        }
    }

    /// Create the process atomically and wake its root operation — or park it,
    /// when every slot is taken (see [`Admissions`]).
    pub(crate) async fn start_process(&self, spec: &ProcessSpec) -> Result<String> {
        // The decision (and the reservation it implies) comes first: it
        // decides the seed — an admitted run's root is `Ready`, a parked one's
        // stays `Created` — and the aggregate is written exactly once. Both
        // happen under one lock, so two concurrent starts cannot take the same
        // last slot.
        let admission = self.admissions.lock().reserve(&spec.process_id);
        let snapshot = match admission {
            Admission::Admit => spec.seed_snapshot(),
            Admission::Park => spec.seed_parked_snapshot(),
        };
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            // A seed that cannot be built must not keep the reserved slot (or
            // park a pid that will never exist).
            Err(error) => {
                self.admissions.lock().abandon(&spec.process_id);
                return Err(error);
            }
        };
        if let Err(error) = self
            .aggregates
            .create(&snapshot)
            .await
            .map_err(repository_error)
        {
            // The reservation never became a run: the slot (or the parked
            // entry) must not outlive the failed creation.
            self.admissions.lock().abandon(&spec.process_id);
            return Err(error);
        }
        // The read model from the first durable fact: the proc row and the
        // seeded task rows exist before the root hop runs, so an immediately
        // following read sees the run. A parked run's proc row reads `None`,
        // exactly like the legacy parked row's.
        self.project(&snapshot).await;
        let root = OperationId {
            process_id: spec.process_id.clone(),
            task_id: spec.root.clone(),
            generation: 0,
            sequence: 0,
        };
        match admission {
            Admission::Admit => self.wake(&root),
            Admission::Park => {
                let created = created_stamp(&snapshot);
                let admitted = {
                    let mut admissions = self.admissions.lock();
                    admissions.park(created, &spec.process_id, &root);
                    // A slot may have freed while this run was being written:
                    // that release found an empty queue, so nothing else would
                    // come back for this run — it takes the slot now.
                    admissions.take_free_slot(created, &spec.process_id)
                };
                if admitted {
                    self.wake(&root);
                } else {
                    tracing::info!(
                        pid = %spec.process_id,
                        "process parked — waiting for a free slot"
                    );
                }
            }
        }
        Ok(spec.process_id.clone())
    }

    /// Wake one operation. A wake is an optimization over the recovery scan:
    /// losing it (a full queue) is safe.
    fn wake(&self, operation: &OperationId) {
        QueueWake {
            tx: self.wake_tx.clone(),
        }
        .wake(operation);
    }

    /// The slot bookkeeping for one run, from a fresh snapshot of it. Returns
    /// whether the hop that asked may run.
    ///
    /// - a run this driver admitted and whose executions all settled frees its
    ///   slot, and the parked run that takes it is woken here;
    /// - a parked run keeps waiting: its operations stay pending and undriven
    ///   until a slot frees;
    /// - a run this driver never admitted — one already in flight when it
    ///   booted, since every start reserves through [`Self::start_process`] —
    ///   takes a slot here, or parks exactly like a start that found every
    ///   slot taken. This is the boot pass: the recovery scan hands every
    ///   unfinished run its first wake, and this is where the cap meets it.
    ///   A parked pid waking that way is admitted by its seed's root
    ///   operation; a run whose root hop has already run may be woken by a
    ///   no-op, and the next recovery tick re-wakes its real work.
    fn place_run(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
    ) -> bool {
        let mut admissions = self.admissions.lock();
        let mut freed = None;
        let drive = if admissions.is_parked(&snapshot.process_id) {
            false
        } else if admissions.holds(&snapshot.process_id) {
            if run_settled(snapshot) {
                freed = admissions.release(&snapshot.process_id);
            }
            true
        } else {
            let state = root_state(snapshot);
            match concurrency::admit(admissions.cap, admissions.active.len(), state) {
                Admission::Admit => {
                    // A settled run is admitted *because* it needs no slot:
                    // taking one here would hold it for a run that never hops
                    // again.
                    if !state.is_terminal() {
                        admissions.activate(&snapshot.process_id);
                    }
                    true
                }
                Admission::Park => {
                    admissions.park(
                        created_stamp(snapshot),
                        &snapshot.process_id,
                        &resume_operation(snapshot),
                    );
                    false
                }
            }
        };
        drop(admissions);
        if let Some(admission) = freed {
            self.wake(&admission);
        }
        drive
    }

    /// Drive woken operations until the queue drains or `max_steps` hops ran.
    /// Test-side twin of [`Self::serve`]: same hop execution, deterministic
    /// drain instead of a select loop.
    #[cfg(test)]
    pub(crate) async fn run_until_quiescent(
        &self,
        wake_rx: &mut Receiver<OperationId>,
        max_steps: usize,
    ) -> Result<RunReport> {
        let mut report = RunReport::default();
        for _ in 0..max_steps {
            let id = match wake_rx.try_recv() {
                Ok(id) => id,
                Err(_) => break,
            };
            match CatchPanic::new(self.execute_one(&id)).await {
                Ok(Ok(true)) => report.executed += 1,
                Ok(Ok(false)) => report.ignored += 1,
                Ok(Err(error)) => {
                    report.retried += 1;
                    #[cfg(test)]
                    eprintln!("hop {id:?} failed: {error}");
                    tracing::debug!(operation = ?id, error = %error, "hop failed; operation stays open for recovery");
                }
                Err(payload) => {
                    report.retried += 1;
                    let message = panic_message(&payload);
                    #[cfg(test)]
                    eprintln!("hop {id:?} panicked: {message}");
                    tracing::error!(operation = ?id, panic = %message, "hop panicked; operation stays open for recovery");
                }
            }
        }
        Ok(report)
    }

    /// Apply a client action to one execution: the durable intent (an
    /// `ApplyAction` operation) is created first, then owned, re-planned
    /// against the authoritative snapshot, committed, and completed.
    pub(crate) async fn apply_action(
        &self,
        process_id: &str,
        task_id: &str,
        action: crate::runtime::core::commit::command::ActionKind,
        inputs: Vec<(String, String)>,
    ) -> Result<()> {
        // The serving loop commits hops of the same aggregate concurrently, so
        // a client's plan can go stale before its commit. A retry re-plans
        // against the fresh snapshot: the action either applies or is
        // legitimately refused — and a refusal is never retried. The retry
        // pins the intent's identity (see [`ActionIntent`]) and consults it
        // first: an attempt that committed its intent and then lost the
        // completion compare-and-swap must report success, never a refusal
        // caused by the state its own commit moved.
        let mut pinned: Option<ActionIntent> = None;
        let mut attempt = 0;
        loop {
            match self
                .apply_action_once(
                    process_id,
                    task_id,
                    action.clone(),
                    inputs.clone(),
                    attempt,
                    &mut pinned,
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if !is_conflict(&error) || attempt + 1 >= ACTION_ATTEMPTS {
                        return Err(error);
                    }
                    if let Some(intent) = &pinned {
                        match self.resume_action_intent(intent).await? {
                            ActionResume::Applied => return Ok(()),
                            ActionResume::Retry => {}
                        }
                    }
                    tracing::debug!(
                        pid = %process_id,
                        tid = %task_id,
                        attempt,
                        "action lost a compare-and-swap; re-planning"
                    );
                    attempt += 1;
                }
            }
        }
    }

    /// The forward half of the legacy cancel guard: the work a cancel rewinds.
    /// The legacy `follows` walk collects the *package* steps ahead of the
    /// cancelled act — the ones that own acts, walking the step *chain* (a
    /// container's children are not `next` tasks there) — and refuses both when
    /// there is none (`cannot find cancelled tasks`) and when every one of them
    /// already settled (`is_completed`, the duplicate-delivery guard: the
    /// rewind it would run was already applied). A specification without a
    /// model — the kernel's linear specs, which the legacy tree cannot express
    /// — keeps the rule's spirit: any forward execution still live is work to
    /// rewind.
    fn validate_cancel_forward_work(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        source: &crate::runtime::core::model::execution_graph::ExecutionId,
    ) -> Result<()> {
        let graph = execution_graph(snapshot)?;
        let modelled = model_of(snapshot)?.is_some();
        let mut seen = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<
            crate::runtime::core::model::execution_graph::ExecutionId,
        > = std::collections::VecDeque::from([source.clone()]);
        let mut forward: Vec<&crate::runtime::core::commit::aggregate::ExecutionSnapshot> =
            Vec::new();
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            for successor in graph.successors(&current) {
                let Some(execution) = snapshot.executions.get(successor) else {
                    continue;
                };
                // A child of `current` is the container's own work, not the
                // chain's next step — the legacy walk follows `next` tasks, so
                // children stay out of the guard. The rewind still recreates
                // them: they belong to the closure the planner rebuilds.
                if execution.parent.as_ref() == Some(&current) {
                    continue;
                }
                let package_step =
                    !modelled || self.step_uses(snapshot, &execution.model_node)?.is_some();
                if package_step {
                    forward.push(execution);
                }
                queue.push_back(successor.clone());
            }
        }
        if forward.is_empty() {
            return Err(ActError::Action("cannot find cancelled tasks".to_string()));
        }
        if forward
            .iter()
            .all(|execution| execution.state.is_terminal())
        {
            return Err(ActError::Action(format!(
                "task('{}') is not allowed to cancel",
                source.task_id
            )));
        }
        Ok(())
    }

    /// The durable intent of one client action: the `ApplyAction` operation
    /// that carries it, plus the action itself as the private fact its hop
    /// replays. Without that fact a recovery driving the operation (a crash,
    /// an expired lease) would plan the bare hop's auto completion instead of
    /// the decision the client made.
    fn action_intent(
        process_id: &str,
        execution: &crate::runtime::core::model::execution_graph::ExecutionId,
        operation: &OperationId,
        action: &crate::runtime::core::commit::command::ActionKind,
        inputs: &[(String, String)],
        expected: crate::runtime::core::model::transition_plan::AggregateVersion,
    ) -> TransitionPlan {
        TransitionPlan {
            process_id: process_id.into(),
            expected,
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: vec![VariableWrite {
                scope: execution.clone(),
                name: action_fact_var(operation.sequence),
                value: action_fact(action, inputs),
            }],
            source_operation: None,
            operations: vec![OperationIntent {
                id: operation.clone(),
                kind: OperationKind::ApplyAction,
            }],
            outbox: Vec::new(),
        }
    }

    /// Whether a retried action's pinned intent already carries its commit —
    /// the action applied, so the retry reports success — and the release of
    /// an intent whose effect never committed (the attempt lost its
    /// compare-and-swap mid-sequence), so the next attempt re-drives the same
    /// operation instead of stranding it.
    async fn resume_action_intent(&self, intent: &ActionIntent) -> Result<ActionResume> {
        let Some(snapshot) = self.load(&intent.operation.process_id).await? else {
            return Ok(ActionResume::Retry);
        };
        let Some(operation) = snapshot.snapshot.operations.get(&intent.operation) else {
            return Ok(ActionResume::Retry);
        };
        match &operation.state {
            // The effect commit is durable, the rewind operation it created
            // included: recovery completes the bookkeeping and drives the
            // rewind, the client's action has applied.
            crate::runtime::core::model::operation::OperationState::Committed(_)
            | crate::runtime::core::model::operation::OperationState::Completed(_) => {
                Ok(ActionResume::Applied)
            }
            crate::runtime::core::model::operation::OperationState::Claimed(claim)
            | crate::runtime::core::model::operation::OperationState::CommitPrepared(claim) => {
                self.operation_event(
                    &intent.operation,
                    operation.revision,
                    OperationEvent::ScheduleRetry {
                        claim: claim.clone(),
                        attempt: claim.fence as u32,
                    },
                )
                .await?;
                Ok(ActionResume::Retry)
            }
            crate::runtime::core::model::operation::OperationState::EffectUnknown { .. }
            | crate::runtime::core::model::operation::OperationState::Quarantined { .. } => {
                Err(runtime(format!(
                    "action intent {:?} needs reconciliation: {:?}",
                    intent.operation, operation.state
                )))
            }
            _ => Ok(ActionResume::Retry),
        }
    }

    /// One action application: the durable intent, ownership, the re-plan
    /// against the authoritative snapshot, the commit and the completion (see
    /// [`Self::apply_action`] for the retry around it). `attempt` fences the
    /// claim (a released intent's next claim must carry a higher fence) and
    /// `pinned` carries the identity the first attempt fixed.
    async fn apply_action_once(
        &self,
        process_id: &str,
        task_id: &str,
        action: crate::runtime::core::commit::command::ActionKind,
        inputs: Vec<(String, String)>,
        attempt: usize,
        pinned: &mut Option<ActionIntent>,
    ) -> Result<()> {
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {process_id} not found")))?
            .snapshot;
        let (execution_id, operation_id) = match pinned {
            Some(intent) => (intent.execution.clone(), intent.operation.clone()),
            None => {
                let execution_id = snapshot
                    .executions
                    .keys()
                    .filter(|id| id.task_id == task_id)
                    .max_by_key(|id| id.generation)
                    .cloned()
                    .ok_or_else(|| ActError::Runtime(format!("execution {task_id} not found")))?;
                let next_sequence = snapshot
                    .operations
                    .keys()
                    .filter(|id| {
                        id.process_id == process_id
                            && id.task_id == task_id
                            && id.generation == execution_id.generation
                    })
                    .map(|id| id.sequence)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let operation_id = OperationId {
                    process_id: process_id.into(),
                    task_id: task_id.into(),
                    generation: execution_id.generation,
                    sequence: next_sequence,
                };
                let intent = ActionIntent {
                    execution: execution_id.clone(),
                    operation: operation_id.clone(),
                };
                *pinned = Some(intent);
                (execution_id, operation_id)
            }
        };
        let now = SystemClock.now_ms();
        let claim = crate::runtime::core::model::operation::ClaimToken {
            owner: OWNER.into(),
            fence: attempt as u64 + 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let command = crate::runtime::core::commit::command::ApplyActionCommand {
            process_id: process_id.into(),
            execution: execution_id.clone(),
            action,
            inputs,
            source_operation: OperationCommit {
                id: operation_id.clone(),
                claim: claim.clone(),
            },
        };

        // Preflight against the current snapshot: a caller-correctable
        // rejection (terminal task, unknown execution) consumes nothing.
        crate::runtime::core::commit::command::ActionPlanner::plan(&snapshot, &command)
            .map_err(runtime)?;
        // A `Back` without the step it names is the legacy refusal, checked
        // before the durable intent: the rewind target is the client's own
        // input, not something the kernel can invent.
        if matches!(
            command.action,
            crate::runtime::core::commit::command::ActionKind::Back
        ) && !command
            .inputs
            .iter()
            .any(|(name, _)| name == crate::utils::consts::ACT_TO)
        {
            return Err(ActError::Runtime(
                "cannot find 'to' value in options".to_string(),
            ));
        }
        // A cancel rewinds the work *forward* of the step the client names:
        // the legacy handler refuses when there is no package step ahead to
        // undo, and when that forward work already settled.
        if matches!(
            command.action,
            crate::runtime::core::commit::command::ActionKind::Cancel
        ) {
            self.validate_cancel_forward_work(&snapshot, &execution_id)?;
        }
        // A step declaring `catches` is not a client target once it is deciding
        // its caught error: the legacy act it folds is already terminal, and the
        // legacy `do_action` refuses every action on it. The durable flow fact
        // is exactly that state — the same hop that dispatched the handlers
        // pinned it — so no child-proxy is needed (a timeout branch the tick
        // fired is a child too, and the step's own act stays actionable while
        // one runs). The refusal is part of the preflight, so it consumes no
        // operation.
        if let Some(model) = model_of(&snapshot)?
            && let Some(node) = snapshot
                .executions
                .get(&execution_id)
                .map(|execution| execution.model_node.as_str())
            && let Some(step) = find_step_in_model(&model, node)
            && !step.catches.is_empty()
            && is_catch_decider(&snapshot, &execution_id)
        {
            return Err(ActError::Action(format!(
                "step '{}' is deciding its catches; the act it folds is decided",
                execution_id.task_id
            )));
        }

        // Durable intent: the operation that carries the action *and* the
        // action itself, as a private fact its hop replays (see
        // `action_intent`).
        let intent = Self::action_intent(
            process_id,
            &execution_id,
            &operation_id,
            &command.action,
            &command.inputs,
            snapshot.version.clone(),
        );
        match self
            .aggregates
            .commit_transition(&intent)
            .await
            .map_err(repository_error)?
        {
            crate::runtime::durable::async_document_store::TransitionCommitOutcome::Committed { .. }
            | crate::runtime::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted { .. } => {}
            crate::runtime::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
                return Err(conflict(format!(
                    "action intent conflict on {process_id}/{task_id}"
                )));
            }
        }

        // Ownership, authoritative re-plan, commit, complete — the standard
        // hop, but planned by ActionPlanner instead of the host's own arms.
        let revision = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during action".into()))?
            .snapshot
            .operations
            .get(&operation_id)
            .map(|record| record.revision)
            .ok_or_else(|| ActError::Runtime("action operation lost".into()))?;
        let (prepared, snapshot) = self
            .operation_event(
                &operation_id,
                revision,
                OperationEvent::ClaimAndPrepare(claim.clone()),
            )
            .await?;
        debug_assert!(matches!(
            prepared.state,
            crate::runtime::core::model::operation::OperationState::CommitPrepared(_)
        ));
        // The claim's snapshot is the authoritative post-claim state this plan
        // is formed from — handed back by the commit, not re-read.
        // The authoritative effect: a container completion fans out instead
        // of completing (see `effect_plan`); every other action is the
        // ActionPlanner's own plan.
        let effect = self
            .effect_plan(
                &snapshot,
                &operation_id,
                &execution_id,
                snapshot.executions.get(&execution_id).ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} lost during action"))
                })?,
                &command.source_operation,
                Some(command.action.clone()),
                command.inputs.clone(),
            )
            .await;
        match effect {
            Ok(plan) => {
                let receipt = match self
                    .aggregates
                    .commit_transition(&plan)
                    .await
                    .map_err(repository_error)?
                {
                    crate::runtime::durable::async_document_store::TransitionCommitOutcome::Committed { receipt, .. }
                    | crate::runtime::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted {
                        receipt, ..
                    } => receipt,
                    crate::runtime::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
                        return Err(conflict(format!(
                            "action commit conflict on {process_id}/{task_id}"
                        )));
                    }
                };
                let committed = self
                    .load(process_id)
                    .await?
                    .ok_or_else(|| ActError::Runtime("aggregate lost after action".into()))?
                    .snapshot
                    .operations
                    .get(&operation_id)
                    .cloned()
                    .ok_or_else(|| ActError::Runtime("action operation lost".into()))?;
                self.operation_event(
                    &operation_id,
                    committed.revision,
                    OperationEvent::Complete {
                        commit: receipt.token.clone(),
                    },
                )
                .await?;
                let wake = QueueWake {
                    tx: self.wake_tx.clone(),
                };
                for operation in &receipt.new_operations {
                    wake.wake(operation);
                }
                // The read model follows the action's commits. A `Cancel` or
                // `Back` re-enters through the durable `ReenterTask` operation
                // the effect commit created (and this wake dispatched), so the
                // rewind survives a lost completion compare-and-swap or a
                // crash exactly like every other hop.
                self.refresh_projection(process_id).await;
                // The action can be what settles the run (a `Complete` on its
                // last execution, an `Abort`): the slot it held goes back to
                // the parked queue here, exactly like the hop that settles a
                // run. Without this a run nobody hops again would keep its
                // slot until the driver's next boot.
                if let Some(snapshot) = self.snapshot(process_id).await? {
                    self.place_run(&snapshot);
                }
                Ok(())
            }
            Err(error) => {
                // The state moved between intent and ownership: park the
                // operation for recovery instead of consuming it.
                let _ = self
                    .operation_event(
                        &operation_id,
                        prepared.revision,
                        OperationEvent::ScheduleRetry {
                            claim: claim.clone(),
                            attempt: 1,
                        },
                    )
                    .await;
                Err(ActError::Runtime(format!("action re-plan failed: {error}")))
            }
        }
    }

    /// One durable hop. Returns `Ok(false)` when the operation needed nothing
    /// (a wake redelivered for an already-terminal operation, or a run parked
    /// by the slot cap).
    async fn execute_one(&self, id: &OperationId) -> Result<bool> {
        // A parked run's operations are re-woken by the recovery scan every
        // tick, so the parked answer comes from the set — a store read would
        // be pure waste on that path.
        if self.admissions.lock().is_parked(&id.process_id) {
            return Ok(false);
        }
        let now = SystemClock.now_ms();
        let snapshot = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        // The slot policy for this run, before it is driven: a parked run
        // waits, a run this driver never admitted (one already in flight when
        // it booted) takes a slot or parks, and a run that settled already —
        // its last hop committed, a client action closed it, or a crash left
        // its timer open — frees its slot for the parked run that has waited
        // longest. The recovery scan re-wakes every pending operation each
        // tick, so this is also the hot path of a parked run.
        if !self.place_run(&snapshot) {
            return Ok(false);
        }
        let record = snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("operation {id:?} not found")))?;
        match record.state {
            OperationState::Pending | OperationState::RetryScheduled { .. } => {}
            _ => return Ok(false),
        }

        // The timeout timer wakes once per tick and usually has nothing to do:
        // evaluating its branches before the claim keeps an idle round from
        // consuming the operation (it stays `Pending`, so the next recovery
        // scan re-wakes it — the kernel's tick cadence) and from writing
        // anything. Only the *idle* verdict short-circuits here; the round is
        // decided again after the claim, against the authoritative snapshot.
        if record.kind == OperationKind::DispatchTimeout {
            let execution_id = crate::runtime::core::model::execution_graph::ExecutionId {
                task_id: id.task_id.clone(),
                generation: id.generation,
            };
            let execution = snapshot.executions.get(&execution_id);
            if matches!(
                self.timeout_round(&snapshot, &execution_id, execution, now)
                    .await?,
                TimeoutRound::Idle
            ) {
                return Ok(false);
            }
        }

        let claim = crate::runtime::core::model::operation::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let (prepared, snapshot) = self
            .operation_event(
                id,
                record.revision,
                OperationEvent::ClaimAndPrepare(claim.clone()),
            )
            .await?;
        debug_assert!(matches!(prepared.state, OperationState::CommitPrepared(_)));

        // The claim's own snapshot is the authoritative post-claim state the
        // plan must be formed from: the commit hands it back instead of the
        // hop decoding the whole aggregate a second time.
        let execution_id = crate::runtime::core::model::execution_graph::ExecutionId {
            task_id: id.task_id.clone(),
            generation: id.generation,
        };
        let execution = snapshot.executions.get(&execution_id).cloned();
        let commit_claim = OperationCommit {
            id: id.clone(),
            claim: crate::runtime::core::model::operation::ClaimToken {
                owner: claim.owner.clone(),
                fence: claim.fence,
                lease_until_ms: claim.lease_until_ms,
                heartbeat_revision: claim.heartbeat_revision,
            },
        };

        let plan = match record.kind {
            OperationKind::ExecuteTask => {
                let action_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let action_exists = snapshot.operations.contains_key(&action_id);
                // Auto tasks seed their own completion action; waiting tasks
                // stay Running until a client action arrives.
                let auto = snapshot
                    .variables
                    .get(&(execution_id.clone(), "auto".into()))
                    .map(String::as_str)
                    != Some("0");
                self.execute_task_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    execution.as_ref(),
                    &commit_claim,
                    &action_id,
                    action_exists,
                    auto,
                    now,
                )
                .await?
            }
            OperationKind::ApplyAction => {
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if !matches!(
                    execution.state,
                    crate::runtime::core::model::execution_graph::ExecutionState::Running
                        | crate::runtime::core::model::execution_graph::ExecutionState::Waiting
                ) {
                    // A decision landed while this hop was in flight (the
                    // cascade skipped the execution, a cancel closed it): the
                    // durable remainder is the empty plan, not an error.
                    self.task_hop_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        Some(&execution),
                        &commit_claim,
                        "completed",
                        "task.completed",
                    )?
                } else if let Some(fact) = snapshot
                    .variables
                    .get(&(execution_id.clone(), action_fact_var(id.sequence)))
                {
                    // The operation carries a client decision (a durable intent
                    // whose deciding call never applied it — a crash, an
                    // expired lease): replay *that* action, exactly like the
                    // deciding call would have. Its `effect_plan` arm is the
                    // same one the client path uses.
                    let (action, inputs) = decode_action_fact(fact)?;
                    self.effect_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        &execution,
                        &commit_claim,
                        Some(action),
                        inputs,
                    )
                    .await?
                } else {
                    // A container fans out instead of completing; a plain task
                    // completes and propagates.
                    self.effect_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        &execution,
                        &commit_claim,
                        None,
                        Vec::new(),
                    )
                    .await?
                }
            }
            OperationKind::SkipTask => {
                // The cascade hop: admit-and-skip already committed
                // `Created → Ready`; this hop marks the execution `Skipped`
                // and hands propagation to a `PropagateTask`, whose readiness
                // treats the skipped source as dead and continues the
                // cascade. Like every hop it is one atomic commit sourced by
                // this operation.
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if !matches!(
                    execution.state,
                    crate::runtime::core::model::execution_graph::ExecutionState::Ready
                        | crate::runtime::core::model::execution_graph::ExecutionState::Running
                        | crate::runtime::core::model::execution_graph::ExecutionState::Waiting
                ) {
                    return Err(ActError::Runtime(format!(
                        "execution {execution_id:?} is {:?}, expected Ready, Running or Waiting",
                        execution.state
                    )));
                }
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let mut plan = self.task_hop_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    Some(&execution),
                    &commit_claim,
                    "skipped",
                    "task.skipped",
                )?;
                // A skipped act emits its Skipped message (the legacy act
                // row's own message).
                if let Some(payload) = self.hop_message_payload(
                    &snapshot,
                    &execution_id,
                    &id.task_id,
                    act_facts(&snapshot, &execution_id).as_ref(),
                    None,
                    None,
                )? {
                    plan.outbox[0].payload = payload.to_string();
                }
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: execution.state,
                    to: crate::runtime::core::model::execution_graph::ExecutionState::Skipped,
                });
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                    });
                }
                plan
            }
            OperationKind::AbortTask => {
                // The abort cascade's running-successor hop: the legacy
                // ancestor walk decides running children `Aborted` (pending
                // ones skip). Same shape as `SkipTask`, terminal `Aborted`.
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if execution.state
                    != crate::runtime::core::model::execution_graph::ExecutionState::Running
                {
                    // A decision landed while the hop was queued: the empty
                    // remainder completes the operation either way.
                    self.task_hop_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        Some(&execution),
                        &commit_claim,
                        "aborted",
                        "task.aborted",
                    )?
                } else {
                    let propagate_id = OperationId {
                        process_id: id.process_id.clone(),
                        task_id: id.task_id.clone(),
                        generation: id.generation,
                        sequence: id.sequence + 1,
                    };
                    let mut plan = self.task_hop_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        Some(&execution),
                        &commit_claim,
                        "aborted",
                        "task.aborted",
                    )?;
                    plan.state_changes.push(StateChange {
                        execution: execution_id.clone(),
                        expected_revision: execution.revision,
                        from: crate::runtime::core::model::execution_graph::ExecutionState::Running,
                        to: crate::runtime::core::model::execution_graph::ExecutionState::Aborted,
                    });
                    // The aborted execution announces itself (the legacy
                    // `on_task` emitted its Aborted message like any other).
                    if let Some(payload) = self.hop_message_payload(
                        &snapshot,
                        &execution_id,
                        &id.task_id,
                        act_facts(&snapshot, &execution_id).as_ref(),
                        None,
                        None,
                    )? {
                        plan.outbox[0].payload = payload.to_string();
                    }
                    if !snapshot.operations.contains_key(&propagate_id) {
                        plan.operations.push(OperationIntent {
                            id: propagate_id,
                            kind: OperationKind::PropagateTask,
                        });
                    }
                    plan
                }
            }
            OperationKind::DispatchTimeout => {
                // The timer of a timed-out execution: the round was already
                // decided before the claim (an idle round never gets here), so
                // this hop either dispatches the due branch, retires a timer
                // with nothing left to fire, or — when a decision landed
                // between the preflight and the claim — re-arms itself for the
                // next tick instead of consuming its own operation.
                match self
                    .timeout_round(&snapshot, &execution_id, execution.as_ref(), now)
                    .await?
                {
                    TimeoutRound::Dispatch {
                        branch,
                        guard,
                        cost_ms,
                        declared,
                    } => self.timeout_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        execution.as_ref(),
                        &commit_claim,
                        branch,
                        guard,
                        cost_ms,
                        declared,
                    )?,
                    // Nothing can fire any more: the owner is decided or its
                    // node declares no timeouts. The operation's own commit is
                    // its whole remainder.
                    TimeoutRound::Retire => Self::empty_remainder(&snapshot, id, &commit_claim),
                    TimeoutRound::Idle => {
                        let _ = self
                            .operation_event(
                                id,
                                prepared.revision,
                                OperationEvent::ScheduleRetry {
                                    claim: claim.clone(),
                                    attempt: 1,
                                },
                            )
                            .await;
                        return Ok(false);
                    }
                }
            }
            OperationKind::PropagateTask => {
                // The graph of record is the snapshot itself (parent map plus
                // predecessor lists), never the declaring spec.
                let graph = execution_graph(&snapshot)?;
                let command = PropagateCommand {
                    process_id: id.process_id.clone(),
                    execution: execution_id.clone(),
                    source_operation: commit_claim,
                    join_policy: crate::runtime::core::plan::propagation::JoinPolicy::AllSucceeded,
                };
                let mut plan =
                    PropagationPlanner::plan(&graph, &snapshot, &command).map_err(runtime)?;
                // A completed execution's own scope data folds into its parent
                // — the legacy `Task::next` → `parent.update_data(&outputs)`:
                // what a step's package wrote is what the step (and, up the
                // chain, the run) answers, which is what a workflow-level
                // `exposes` judges and what `proc:get`'s data shows. Engine
                // facts and anything this hop already writes for that scope
                // are left out (the failure path owns its own error
                // variables).
                if execution.as_ref().map(|e| e.state)
                    == Some(crate::runtime::core::model::execution_graph::ExecutionState::Completed)
                    && let Some(parent) = execution.as_ref().and_then(|e| e.parent.clone())
                {
                    let mut carried: Vec<(String, String)> = Vec::new();
                    for ((scope, name), raw) in &snapshot.variables {
                        if scope != &execution_id || is_engine_fact(name) {
                            continue;
                        }
                        let already = plan
                            .variable_writes
                            .iter()
                            .any(|write| write.scope == parent && &write.name == name);
                        if !already && !carried.iter().any(|(existing, _)| existing == name) {
                            carried.push((name.clone(), raw.clone()));
                        }
                    }
                    for (name, value) in carried {
                        plan.variable_writes.push(VariableWrite {
                            scope: parent.clone(),
                            name,
                            value,
                        });
                    }
                }
                // The decision walks' upward half: an aborted execution ends
                // its whole ancestor chain `Aborted` (legacy `abort_task`),
                // a failed one ends it `Failed` (legacy `emit_error` →
                // `parent.set_err`); each newly decided ancestor's own hop
                // carries the death into its remaining subtrees.
                let upward_to = match execution.as_ref().map(|e| e.state) {
                    Some(crate::runtime::core::model::execution_graph::ExecutionState::Aborted) => {
                        Some(crate::runtime::core::model::execution_graph::ExecutionState::Aborted)
                    }
                    Some(crate::runtime::core::model::execution_graph::ExecutionState::Failed) => {
                        Some(crate::runtime::core::model::execution_graph::ExecutionState::Failed)
                    }
                    _ => None,
                };
                // The container join: a parent parked by a fan-out completes
                // in the same atomic commit that settles its last child; a
                // failed step's CATCH child runs the catch join instead, which
                // either catches the error or resumes it. A decision walk
                // already decides the parent itself (legacy calls
                // `parent.set_err` before the auto-complete pass sees the
                // children settled), so both yield to it — the same change
                // twice is not a plan.
                if upward_to.is_none()
                    && let Some(parent) = execution.as_ref().and_then(|e| e.parent.clone())
                {
                    let catch_join =
                        crate::runtime::core::plan::container::ContainerPlanner::plan_catch_join(
                            &graph,
                            &snapshot,
                            &crate::runtime::core::plan::container::CatchJoinCommand {
                                process_id: id.process_id.clone(),
                                parent: parent.clone(),
                                source: execution_id.clone(),
                            },
                        )
                        .map_err(runtime)?;
                    match catch_join {
                        Some(crate::runtime::core::plan::container::CatchJoinOutcome::Resumed(
                            change,
                            operation,
                        )) => {
                            // The record is keyed by the operation that decides
                            // the step — the durable-fact identity the commit
                            // requires (`failed:` keys never repeat).
                            let record_key = format!(
                                "failed:{}:{}:{}:{}",
                                snapshot.process_id,
                                parent.task_id,
                                parent.generation,
                                operation.id.sequence
                            );
                            plan.state_changes.push(change);
                            plan.operations.push(operation);
                            // The step's own failure message: the legacy walk
                            // re-derived the error and emitted the step's task
                            // event (`emit_error`), which is what tells the
                            // client the handlers caught nothing.
                            let code = execution_variable(&snapshot, &parent, "error.code")
                                .map(json_text)
                                .unwrap_or_default();
                            let text = execution_variable(&snapshot, &parent, "error.message")
                                .map(json_text)
                                .unwrap_or_default();
                            if let Some(payload) = self.hop_message_payload(
                                &snapshot,
                                &parent,
                                &parent.task_id,
                                act_facts(&snapshot, &parent).as_ref(),
                                None,
                                Some((&code, &text)),
                            )? {
                                plan.outbox.push(
                                    crate::runtime::core::model::transition_plan::OutboxIntent {
                                        key: record_key,
                                        topic: "task.failed".into(),
                                        payload: payload.to_string(),
                                    },
                                );
                            }
                        }
                        Some(crate::runtime::core::plan::container::CatchJoinOutcome::Caught(
                            change,
                            operation,
                        )) => {
                            self.attach_join_message(&snapshot, &mut plan, &parent, &operation)?;
                            plan.state_changes.push(change);
                            plan.operations.push(operation);
                        }
                        None => {
                            let join =
                                crate::runtime::core::plan::container::ContainerJoinCommand {
                                    process_id: id.process_id.clone(),
                                    parent,
                                };
                            if let Some((change, operation)) =
                                crate::runtime::core::plan::container::ContainerPlanner::plan_join(
                                    &graph, &snapshot, &join,
                                )
                                .map_err(runtime)?
                            {
                                self.attach_join_message(
                                    &snapshot,
                                    &mut plan,
                                    &join.parent,
                                    &operation,
                                )?;
                                plan.state_changes.push(change);
                                plan.operations.push(operation);
                            }
                        }
                    }
                }
                if let Some(to) = upward_to {
                    let (changes, operations) =
                        crate::runtime::core::plan::upward::UpwardPlanner::plan_upward(
                            &snapshot,
                            &crate::runtime::core::plan::upward::UpwardCommand {
                                process_id: id.process_id.clone(),
                                source: execution_id.clone(),
                                to,
                            },
                        )
                        .map_err(runtime)?;
                    // The error the decision carries travels up with it: the
                    // legacy `emit_error` → `parent.set_err` wrote the same
                    // code/message into every ancestor it decided, so an
                    // ancestor's own handler reads `$ecode()`, and its error
                    // row names what actually failed — instead of a bare
                    // `Failed`. An abort carries no error text.
                    if matches!(
                        to,
                        crate::runtime::core::model::execution_graph::ExecutionState::Failed
                    ) {
                        let carried = ["error.code", "error.message", "ecode", "message"]
                            .into_iter()
                            .filter_map(|name| {
                                execution_variable(&snapshot, &execution_id, name)
                                    .map(|value| (name.to_string(), value.to_string()))
                            })
                            .collect::<Vec<_>>();
                        for change in &changes {
                            for (name, value) in &carried {
                                plan.variable_writes.push(VariableWrite {
                                    scope: change.execution.clone(),
                                    name: name.clone(),
                                    value: value.clone(),
                                });
                            }
                        }
                    }
                    plan.state_changes.extend(changes);
                    plan.operations.extend(operations);
                }
                plan.validate().map_err(runtime)?;
                plan
            }
            OperationKind::ReenterTask => {
                // The durable rewind: the deciding commit closed the source
                // generation and wrote the target it names (`Back` an earlier
                // step, `Cancel` the closed execution itself). Planning here —
                // not in the deciding call — is what makes the rewind survive
                // a lost compare-and-swap or a crash: this hop is re-driven by
                // the recovery scan until its commit lands.
                let target = snapshot
                    .variables
                    .get(&(execution_id.clone(), REENTER_TARGET_VAR.to_string()))
                    .cloned()
                    .unwrap_or_else(|| id.task_id.clone());
                let graph = execution_graph(&snapshot)?;
                let command = crate::runtime::core::plan::generation::ReenterCommand {
                    process_id: id.process_id.clone(),
                    source: execution_id.clone(),
                    target_task_id: target,
                    source_operation: commit_claim.clone(),
                };
                crate::runtime::core::plan::generation::GenerationPlanner::plan_reentry(
                    &graph, &snapshot, &command,
                )
                .map_err(runtime)?
            }
            other => {
                return Err(ActError::Runtime(format!(
                    "operation kind {other:?} is not driven by the host yet"
                )));
            }
        };

        let (receipt, loaded) = match self
            .aggregates
            .commit_transition(&plan)
            .await
            .map_err(repository_error)?
        {
            crate::runtime::durable::async_document_store::TransitionCommitOutcome::Committed { receipt, snapshot }
            | crate::runtime::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted { receipt, snapshot } => (receipt, snapshot),
            crate::runtime::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
                // Another writer advanced the aggregate; the wake stays open
                // and recovery re-derives the hop against the new state.
                return Err(ActError::Runtime(format!(
                    "commit conflict on operation {id:?}"
                )));
            }
        };
        // The commit's own snapshot is the committed state: read the operation
        // straight off it instead of decoding the aggregate again.
        let committed = loaded
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime("operation lost after commit".into()))?;
        self.operation_event(
            id,
            committed.revision,
            OperationEvent::Complete {
                commit: receipt.token.clone(),
            },
        )
        .await?;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        for operation in &receipt.new_operations {
            // A timer armed by an execution's *start* hop waits for the tick:
            // the legacy engine had no round before its first process tick, so
            // waking it here would let an already-open window fire up to one
            // tick early. Waking it is the recovery scan's job (it re-wakes
            // every pending operation on each tick). A timer a *round* re-armed
            // (the branch chain of a timed-out execution) is woken at once:
            // that is the legacy `Step::next` handing the turn to the branch
            // declared after the one that just ran.
            if record.kind == OperationKind::ExecuteTask
                && loaded
                    .operations
                    .get(operation)
                    .is_some_and(|created| created.kind == OperationKind::DispatchTimeout)
            {
                continue;
            }
            wake.wake(operation);
        }
        // The run's lifecycle, from the freshly committed state: a start while
        // it is alive, one terminal event once every execution settled. A
        // closed sink or an absent one makes this a no-op.
        self.report_lifecycle(&loaded);
        // The read model follows the same commit — only the executions the
        // plan touched (their rows are the only ones that can have changed).
        let touched: std::collections::BTreeSet<String> = plan
            .state_changes
            .iter()
            .map(|change| change.execution.task_id.clone())
            .chain(
                plan.execution_creates
                    .iter()
                    .map(|create| create.execution.task_id.clone()),
            )
            .chain(
                plan.variable_writes
                    .iter()
                    .map(|write| write.scope.task_id.clone()),
            )
            .collect();
        self.project_touched(&loaded, &touched).await;
        // This hop may have settled the run: its slot goes back to the parked
        // queue right here, so the run that has waited longest starts now
        // instead of on the driver's next tick. The run is the one this hop
        // just drove, so it either holds its slot or needs none — never parked.
        self.place_run(&loaded);
        Ok(true)
    }

    /// The `ExecuteTask` hop: `Ready → Running` (auto) or `Ready → Waiting` (a
    /// waiting package), the act's start-time package resolution — the legacy
    /// act's `run()` validates params before it either waits or executes — the
    /// `Created` message record and, for an auto step, the `ApplyAction`
    /// operation. A resolution failure returns the failure plan instead (the
    /// execution decided `Failed`, the error walk taking over).
    #[allow(clippy::too_many_arguments)]
    async fn execute_task_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: Option<&crate::runtime::core::commit::aggregate::ExecutionSnapshot>,
        claim: &OperationCommit,
        action_id: &OperationId,
        action_exists: bool,
        auto: bool,
        now_ms: i64,
    ) -> Result<TransitionPlan> {
        let mut plan = self.task_hop_plan(
            snapshot,
            id,
            execution_id,
            execution,
            claim,
            "started",
            "task.started",
        )?;
        // A branch whose predicate could not be evaluated fails on its first
        // hop: the fan-out admitted it with the failure pinned (see
        // `fanout_command`), which is the legacy branch task erroring in its
        // `init` — the container's error walk takes over from here.
        if let Some(execution) = execution
            && let Some(failure) = branch_error(snapshot, execution_id)
        {
            return self
                .failure_plan(snapshot, id, execution_id, execution, claim, &failure)
                .await;
        }
        // The act's package facts and its own gate: every `uses` step resolves
        // its package at start, and the pinned facts ride every later message
        // record — so no later hop re-runs the headless scope, whose snapshot
        // seal must fail the act at start, never a completion.
        let created_key = format!(
            "message.created:{}:{}:{}",
            id.process_id, id.task_id, id.generation
        );
        let created_exists = snapshot
            .outbox
            .keys()
            .any(|outbox_id| outbox_id.key == created_key);
        // Whether the acting node is a `uses` step at all: the message record
        // exists for every package step, with or without a runner attached
        // (the payload then carries the raw declared params).
        let has_package = match execution {
            Some(execution) => self.step_uses(snapshot, &execution.model_node)?.is_some(),
            None => false,
        };
        let (facts, fresh) = if !has_package || created_exists {
            (act_facts(snapshot, execution_id), false)
        } else if let Some(execution) = execution {
            match self
                .resolve_step(snapshot, execution_id, &execution.model_node)
                .await
            {
                Ok(facts) => (facts, true),
                Err(error) => {
                    // The act cannot start with these params (the legacy act's
                    // `run()` fails it before it waits or executes). The state
                    // table, like the legacy sequence, only decides a failure
                    // from a started act: this hop marks it started and hands
                    // the decision to the effect hop, which resolves again and
                    // fails it — the same error walk, one hop later.
                    tracing::debug!(
                        pid = %snapshot.process_id,
                        node = %execution_id.task_id,
                        error = %error,
                        "act resolution failed at start; failing it on its effect hop"
                    );
                    if startable(execution.state) {
                        plan.state_changes.push(StateChange {
                            execution: execution_id.clone(),
                            expected_revision: execution.revision,
                            from: execution.state,
                            to: crate::runtime::core::model::execution_graph::ExecutionState::Running,
                        });
                        if !action_exists {
                            plan.operations.push(OperationIntent {
                                id: action_id.clone(),
                                kind: OperationKind::ApplyAction,
                            });
                        }
                    }
                    return Ok(plan);
                }
            }
        } else {
            (None, false)
        };
        // The act message: every `uses` step announces itself to the client
        // when it starts (the legacy act's `Created` message — Irq waits on
        // it, Msg informs, Func pairs it with the later completion). The
        // durable record rides the outbox; the attached transport turns it
        // into the engine's message.
        if has_package
            && let Some(facts) = &facts
            && fresh
        {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: PARAMS_VAR.to_string(),
                value: facts.params.to_string(),
            });
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: OPTIONS_VAR.to_string(),
                value: facts.options.to_string(),
            });
        }
        // The execution announces itself: the act's Created message for a
        // `uses` step, the task-level one (type = the node kind) for a
        // package-less execution — the legacy engine's `on_task` fired for
        // every task, not only the acts.
        if !created_exists
            && let Some(payload) = self.hop_message_payload(
                snapshot,
                execution_id,
                &id.task_id,
                facts.as_ref(),
                None,
                None,
            )?
        {
            plan.outbox
                .push(crate::runtime::core::model::transition_plan::OutboxIntent {
                    key: created_key,
                    topic: "message.created".into(),
                    payload: payload.to_string(),
                });
        }
        if !action_exists && auto {
            plan.operations.push(OperationIntent {
                id: action_id.clone(),
                kind: OperationKind::ApplyAction,
            });
        }
        if let Some(starting) = execution.filter(|starting| startable(starting.state)) {
            // Waiting packages park in `Waiting` (the legacy `Interrupt`);
            // everything else runs to completion.
            let target = if auto {
                crate::runtime::core::model::execution_graph::ExecutionState::Running
            } else {
                crate::runtime::core::model::execution_graph::ExecutionState::Waiting
            };
            plan.state_changes.push(StateChange {
                execution: execution_id.clone(),
                expected_revision: starting.revision,
                from: starting.state,
                to: target,
            });
            // A node that declares `timeouts` starts its cost clock here and
            // arms the timer that fires them (see `crate::runtime::timeout` and
            // `KernelHost::timeout_round`): the timer is durable, so a tick
            // after a crash still fires a branch whose window opened while the
            // host was down. Only timeout owners carry the stamp — it is a
            // kernel fact, never a variable a model reads.
            if !self
                .timeout_branches(snapshot, &starting.model_node)?
                .is_empty()
            {
                plan.variable_writes
                    .push(crate::runtime::core::plan::timeout::start_write(
                        execution_id,
                        now_ms,
                    ));
                let sequence = next_plan_sequence(
                    snapshot,
                    &plan,
                    &execution_id.task_id,
                    execution_id.generation,
                );
                plan.operations.push(OperationIntent {
                    id: OperationId {
                        process_id: id.process_id.clone(),
                        task_id: execution_id.task_id.clone(),
                        generation: execution_id.generation,
                        sequence,
                    },
                    kind: OperationKind::DispatchTimeout,
                });
            }
        }
        Ok(plan)
    }

    /// The bare hop plan: the operation's own commit plus its outbox note.
    /// State changes and successor operations are layered on by the callers.
    #[allow(clippy::too_many_arguments)]
    fn task_hop_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: Option<&crate::runtime::core::commit::aggregate::ExecutionSnapshot>,
        claim: &OperationCommit,
        note: &str,
        topic: &str,
    ) -> Result<TransitionPlan> {
        execution
            .ok_or_else(|| ActError::Runtime(format!("execution {execution_id:?} not found")))?;
        Ok(TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: vec![crate::runtime::core::model::transition_plan::OutboxIntent {
                key: format!("{note}:{}:{}:{}", id.process_id, id.task_id, id.generation),
                topic: topic.into(),
                payload: String::new(),
            }],
        })
    }

    /// The durable effect of one act completion. A plain execution completes
    /// and propagates; a container (a branched step, or a branch over its own
    /// steps) fans its children out instead and parks in `Waiting` until the
    /// join completes it. `action` is `None` for the auto-completion hop.
    /// Only a completion fans out — every other client decision (Submit /
    /// Skip / Cancel / Abort / Fail) is terminal for the container and its
    /// unborn children, exactly like the legacy engine, where a step that is
    /// no longer running never schedules its branches.
    ///
    /// On the auto-completion hop an attached effect runner runs the step's
    /// Func package (the legacy `ActRunAs::Func` run): outputs commit as
    /// scope variables with the same atomic completion, and a package error
    /// decides the execution `Failed` with the error variables — the legacy
    /// package-error path, whose error walk the propagation then carries.
    // Like `task_hop_plan`, the planner mirrors the aggregate's own facts
    // (operation, execution, claim) as separate parameters; bundling them
    // into a struct would only rename the same eight fields.
    #[allow(clippy::too_many_arguments)]
    async fn effect_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: &crate::runtime::core::commit::aggregate::ExecutionSnapshot,
        claim: &OperationCommit,
        action: Option<crate::runtime::core::commit::command::ActionKind>,
        inputs: Vec<(String, String)>,
    ) -> Result<TransitionPlan> {
        // The step is already deciding a caught error (the sign its failure
        // hop pinned): no later hop may complete, fail or fan it out — the
        // catch join decides it. The client path refuses such an action at the
        // preflight; what reaches here is the stale auto hop a failure raced,
        // which keeps the operation's own commit as its bare remainder (the
        // same treatment the `ApplyAction` arm gives a decision that landed
        // mid-hop).
        if is_catch_decider(snapshot, execution_id) {
            return self.task_hop_plan(
                snapshot,
                id,
                execution_id,
                Some(execution),
                claim,
                "completed",
                "task.completed",
            );
        }
        match action {
            None => {
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                // The act's own effect runs *before* its children are
                // dispatched: the legacy act completes (or fails) first, and
                // a `catches` handler answers the act's failure — never a
                // failure of the children it already dispatched.
                let facts = act_facts(snapshot, execution_id);
                let mut effect: Option<effects::EffectOutcome> = None;
                if let Some((runner, request)) =
                    self.step_effect(snapshot, execution_id, &execution.model_node)?
                {
                    match runner.run(request).await {
                        Ok(outcome) => effect = Some(outcome),
                        Err(error) => {
                            // A package error takes the failure path, where a
                            // step declaring `catches` keeps running and
                            // dispatches them (the legacy `Step::on_error`).
                            if let Some(plan) = self
                                .catch_fanout(
                                    snapshot,
                                    claim,
                                    execution_id,
                                    execution,
                                    &package_failure(&error),
                                    &inputs,
                                )
                                .await?
                            {
                                return Ok(plan);
                            }
                            return self
                                .package_failure_plan(
                                    snapshot,
                                    id,
                                    execution_id,
                                    execution,
                                    claim,
                                    &error,
                                )
                                .await;
                        }
                    }
                }
                // A container dispatches its children instead of completing —
                // with the act's own outcome folded into the same atomic
                // commit. A leaf completes and propagates.
                if let Some(command) = self
                    .fanout_command(
                        snapshot,
                        execution_id,
                        claim,
                        &execution.model_node,
                        &inputs,
                    )
                    .await?
                {
                    let mut plan = self.container_fanout_plan(snapshot, &command)?;
                    if let Some(outcome) = &effect {
                        self.fold_effect_outcome(snapshot, execution_id, outcome, &mut plan);
                        plan.validate().map_err(runtime)?;
                    }
                    return Ok(plan);
                }
                // A step whose timeout branches are still in flight may not
                // complete yet: the legacy `auto_complete` counts every child
                // task, the timeout branches included. The act's own completion
                // is recorded and the execution waits for them (the container
                // join finishes it when the last one settles).
                if self.waits_for_timeouts(snapshot, execution_id, execution)? {
                    let mut plan = self.park_for_children_plan(
                        snapshot,
                        id,
                        execution_id,
                        execution,
                        claim,
                        &inputs,
                    )?;
                    if let Some(outcome) = &effect {
                        self.fold_effect_outcome(snapshot, execution_id, outcome, &mut plan);
                        plan.validate().map_err(runtime)?;
                    }
                    return Ok(plan);
                }
                let mut plan = self.task_hop_plan(
                    snapshot,
                    id,
                    execution_id,
                    Some(execution),
                    claim,
                    "completed",
                    "task.completed",
                )?;
                if let Some(outcome) = &effect {
                    self.fold_effect_outcome(snapshot, execution_id, outcome, &mut plan);
                }
                // The act's Completed message rides the completion record.
                if let Some(payload) = self.hop_message_payload(
                    snapshot,
                    execution_id,
                    &id.task_id,
                    facts.as_ref(),
                    Some(&plan.variable_writes),
                    None,
                )? {
                    plan.outbox[0].payload = payload.to_string();
                }
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: execution.state,
                    to: crate::runtime::core::model::execution_graph::ExecutionState::Completed,
                });
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                    });
                }
                Ok(plan)
            }
            Some(kind) => {
                // A client completion of a container dispatches its children
                // instead of completing it (the legacy `Next` on a running
                // step) — the step declaring `catches` included: the completion
                // is its *act* completing, which then runs into its handlers.
                if matches!(
                    kind,
                    crate::runtime::core::commit::command::ActionKind::Complete
                ) && let Some(command) = self
                    .fanout_command(
                        snapshot,
                        execution_id,
                        claim,
                        &execution.model_node,
                        &inputs,
                    )
                    .await?
                {
                    return self.container_fanout_plan(snapshot, &command);
                }
                // A completion of a step whose timeout branches are still in
                // flight is its *act* completing, not the step: the execution
                // waits for the branches it fired (see `park_for_children_plan`).
                if matches!(
                    kind,
                    crate::runtime::core::commit::command::ActionKind::Complete
                ) && self.waits_for_timeouts(snapshot, execution_id, execution)?
                {
                    return self.park_for_children_plan(
                        snapshot,
                        id,
                        execution_id,
                        execution,
                        claim,
                        &inputs,
                    );
                }
                // A client's error on a step declaring `catches` is the legacy
                // `Step::on_error`: the step stays running and dispatches its
                // handlers instead of being decided `Failed` here.
                if let crate::runtime::core::commit::command::ActionKind::Fail { code, message } =
                    &kind
                    && let Some(plan) = self
                        .catch_fanout(
                            snapshot,
                            claim,
                            execution_id,
                            execution,
                            &Failure::new(code.clone(), message.clone()),
                            &inputs,
                        )
                        .await?
                {
                    return Ok(plan);
                }
                let rewinds = matches!(
                    kind,
                    crate::runtime::core::commit::command::ActionKind::Cancel
                        | crate::runtime::core::commit::command::ActionKind::Back
                );
                let back = matches!(
                    kind,
                    crate::runtime::core::commit::command::ActionKind::Back
                );
                let command = crate::runtime::core::commit::command::ApplyActionCommand {
                    process_id: id.process_id.clone(),
                    execution: execution_id.clone(),
                    action: kind,
                    inputs,
                    source_operation: claim.clone(),
                };
                let mut plan =
                    crate::runtime::core::commit::command::ActionPlanner::plan(snapshot, &command)
                        .map_err(runtime)?;
                // A `Cancel`/`Back` rewinds: the decision closes the source
                // generation and the re-entry that follows is a durable
                // operation created in this same commit, so the rewind can be
                // neither lost to a crash nor confused with a state the
                // client never decided. The target it names is a durable fact
                // too (`Back` the client's `to`, `Cancel` this execution).
                if rewinds {
                    let target = if back {
                        command
                            .inputs
                            .iter()
                            .find(|(name, _)| name == crate::utils::consts::ACT_TO)
                            .map(|(_, value)| value.clone())
                            .ok_or_else(|| {
                                ActError::Runtime("cannot find 'to' value in options".to_string())
                            })?
                    } else {
                        execution_id.task_id.clone()
                    };
                    plan.variable_writes.push(VariableWrite {
                        scope: execution_id.clone(),
                        name: REENTER_TARGET_VAR.to_string(),
                        value: target,
                    });
                    let reenter_id = OperationId {
                        process_id: id.process_id.clone(),
                        task_id: execution_id.task_id.clone(),
                        generation: execution_id.generation,
                        sequence: id.sequence + 2,
                    };
                    if !snapshot.operations.contains_key(&reenter_id) {
                        plan.operations.push(OperationIntent {
                            id: reenter_id,
                            kind: OperationKind::ReenterTask,
                        });
                    }
                    plan.validate().map_err(runtime)?;
                }
                // A client action that decides a `uses` execution announces it
                // on the act's own message, exactly like the legacy act.
                self.attach_hop_message(snapshot, execution_id, &mut plan)?;
                Ok(plan)
            }
        }
    }

    /// The `(uses, raw params, node name, model id)` of a `uses` step node,
    /// from the durable model — the act message's and the effect seam's
    /// package facts.
    fn step_uses(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        model_node: &crate::runtime::core::model::model_graph::NodeId,
    ) -> Result<Option<(String, serde_json::Value, String, String)>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(None);
        };
        let Some(uses) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some((
            uses,
            step.params.clone(),
            step.name.clone(),
            model.id.clone(),
        )))
    }

    /// The act message payload of one `uses` step: the fact set the driver's
    /// transport maps onto the engine's `Message`, faithful to the legacy
    /// `Task::create_message` — the resolved params and options, the step and
    /// model inputs, the scope inputs the message carries, then the hop's own
    /// outputs or its error. `facts` are the start hop's pinned resolution
    /// (absent for a host without a runner or an aggregate seeded before it);
    /// `outputs` are the hop's own scope writes (`__`-private facts and the
    /// error variables excluded); `error` is the failure the hop decided.
    #[allow(clippy::too_many_arguments)]
    fn act_message_payload(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        task_id: &str,
        facts: Option<&ActFacts>,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        let node =
            crate::runtime::core::model::model_graph::NodeId::parse(task_id).map_err(runtime)?;
        let Some((uses, raw_params, name, mid)) = self.step_uses(snapshot, &node)? else {
            return Ok(None);
        };
        // The resolved facts when the start hop pinned them, the raw
        // declaration otherwise (`params` before a runner ever saw the step).
        let params = facts
            .map(|facts| facts.params.clone())
            .unwrap_or(raw_params);
        let options = facts
            .map(|facts| facts.options.clone())
            .unwrap_or(serde_json::Value::Null);

        let mut inputs = input_vars(snapshot, execution_id);
        // The act folds into its step node in the kernel, so the step the
        // legacy message pointed at is the node itself.
        inputs.insert(
            crate::utils::consts::STEP_KEY.to_string(),
            serde_json::json!({
                crate::utils::consts::STEP_NODE_ID: task_id,
                crate::utils::consts::STEP_NODE_NAME: name,
                crate::utils::consts::STEP_TASK_ID: task_id,
            }),
        );
        if let Some(model) = model_of(snapshot)? {
            inputs.insert(
                crate::utils::consts::WORKFLOW_MODEL_KEY.to_string(),
                serde_json::json!({
                    "id": model.id,
                    "name": model.name,
                    "options": model.options,
                }),
            );
        }
        inputs.insert(crate::utils::consts::ACT_PARAMS_KEY.to_string(), params);
        inputs.insert(crate::utils::consts::ACT_OPTIONS_KEY.to_string(), options);

        let mut payload = serde_json::json!({
            "nid": task_id,
            "name": name,
            "uses": uses,
            "mid": mid,
            "inputs": serde_json::Value::Object(inputs),
        });
        if let Some((code, message)) = error {
            payload["error"] = serde_json::json!({ "code": code, "message": message });
        } else if let Some(outputs) = outputs {
            payload["outputs"] = message_outputs(outputs, execution_id);
        }
        Ok(Some(payload))
    }

    /// The task-level message payload of one execution *without* a package: the
    /// fact set the driver's transport maps onto the engine's `Message`. The
    /// legacy engine announced every task it created, settled or skipped — not
    /// only the acts (its `on_task` emits each state change that is neither
    /// `pending` nor `running`) — with `type` = the node kind and `name` the
    /// node's name.
    fn task_message_payload(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        task_id: &str,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let (kind, name) = node_kind_and_name(&model, task_id);
        // A branch task emits nothing: the legacy `Branch::init` clears its
        // emit flag and never re-enables it (only an act's `run` does), so a
        // branch's own state changes are invisible to clients on both engines.
        if kind == "branch" {
            return Ok(None);
        }
        let mut inputs = input_vars(snapshot, execution_id);
        // The workflow model rides every legacy message; a package-less task
        // carries no params and no `step` link of its own.
        inputs.insert(
            crate::utils::consts::WORKFLOW_MODEL_KEY.to_string(),
            serde_json::json!({
                "id": model.id,
                "name": model.name,
                "options": model.options,
            }),
        );
        let mut payload = serde_json::json!({
            "nid": task_id,
            "name": name,
            "kind": kind,
            "inputs": serde_json::Value::Object(inputs),
        });
        if let Some((code, message)) = error {
            payload["error"] = serde_json::json!({ "code": code, "message": message });
        } else if let Some(outputs) = outputs {
            payload["outputs"] = message_outputs(outputs, execution_id);
        }
        Ok(Some(payload))
    }

    /// The message payload of one hop's record: the act payload for a `uses`
    /// step, the task payload otherwise. Both are the same legacy emission
    /// (`Task::create_message`); only the fact set differs.
    fn hop_message_payload(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        task_id: &str,
        facts: Option<&ActFacts>,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        if let Some(payload) =
            self.act_message_payload(snapshot, execution_id, task_id, facts, outputs, error)?
        {
            return Ok(Some(payload));
        }
        self.task_message_payload(snapshot, execution_id, task_id, outputs, error)
    }

    /// The variable space the evaluation seams see for one execution: its
    /// scope chain (nearest wins), the run's durable env and its caller
    /// authority, on the acting node of the declaring model.
    fn scope_spec(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        model: crate::model::Workflow,
    ) -> Result<scope::ScopeSpec> {
        Ok(scope::ScopeSpec {
            process_id: snapshot.process_id.clone(),
            task_id: execution_id.task_id.clone(),
            model,
            vars: scope_vars(snapshot, execution_id),
            env: projection::env_map(snapshot)?,
            owner: owner_of(snapshot)?,
        })
    }

    /// The effect request for a `uses` step, when a runner is attached:
    /// `None` for a leaf without a package or a host running without the seam.
    fn step_effect(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        model_node: &crate::runtime::core::model::model_graph::NodeId,
    ) -> Result<Option<(Arc<dyn effects::EffectRunner>, effects::EffectRequest)>> {
        let Some(runner) = self.effect.read().clone() else {
            return Ok(None);
        };
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(None);
        };
        let Some(package) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some((
            runner,
            effects::EffectRequest {
                scope: self.scope_spec(snapshot, execution_id, model)?,
                package,
            },
        )))
    }

    /// Resolve one `uses` step's package at start (the legacy act's `run()`
    /// gate): the package's `run_as` and its validated params. A resolution
    /// error is the legacy act's start error.
    async fn resolve_step(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        model_node: &crate::runtime::core::model::model_graph::NodeId,
    ) -> Result<Option<ActFacts>> {
        let Some((runner, request)) = self.step_effect(snapshot, execution_id, model_node)? else {
            return Ok(None);
        };
        let resolved = runner.resolve(&request).await?;
        Ok(Some(ActFacts {
            params: resolved.params,
            options: resolved.options,
        }))
    }

    /// The declared timeout branches of one model node, in declaration order.
    ///
    /// An empty list means the node's step declares no `timeouts`: the timer
    /// has nothing to fire (`Retire`). Each branch carries the durable facts a
    /// dispatch needs; its guard stays in the model and is evaluated by the
    /// round.
    fn timeout_branches(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        model_node: &crate::runtime::core::model::model_graph::NodeId,
    ) -> Result<Vec<crate::runtime::core::plan::timeout::TimeoutBranch>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(Vec::new());
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(Vec::new());
        };
        let mut branches = Vec::with_capacity(step.timeouts.len());
        for timeout in &step.timeouts {
            branches.push(crate::runtime::core::plan::timeout::TimeoutBranch {
                task_id: timeout.id.clone(),
                model_node: node_id(&timeout.id)?,
                vars: timeout
                    .vars
                    .iter()
                    .map(|var| (var.name.clone(), var.value.to_string()))
                    .collect(),
                auto: auto_decision(snapshot, &timeout.id)?,
            });
        }
        Ok(branches)
    }

    /// The `if` of one declared branch, from the declaring model: the window the
    /// tick evaluates (`$cost_in(...)` in the models this ports). A branch
    /// without a guard fires as soon as the round reaches it — a legacy branch
    /// whose `Step::init` had nothing to test.
    fn timeout_guard(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        branch: &str,
    ) -> Result<Option<String>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        Ok(find_step_in_model(&model, branch).and_then(|step| step.r#if.clone()))
    }

    /// One dispatch round of a timed-out execution: the legacy tick's
    /// `Step::on_timeout` + `dispatch_next_timeout_branch` + the branch's own
    /// `Step::init` guard, decided against the aggregate.
    ///
    /// The cost is what passed since the execution's own start hop (the legacy
    /// task's creation stamp); it is frozen into the branch's evaluation as the
    /// `__cost` task datum `$cost()`/`$cost_in(...)` read.
    async fn timeout_round(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        owner: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: Option<&crate::runtime::core::commit::aggregate::ExecutionSnapshot>,
        now_ms: i64,
    ) -> Result<TimeoutRound> {
        // Only a running or waiting execution times out (the legacy tick's
        // `is_timeouts() && (running || interrupted)`); anything else — a
        // re-entered generation's stale timer included — has nothing left.
        let Some(execution) = execution.filter(|execution| {
            matches!(
                execution.state,
                crate::runtime::core::model::execution_graph::ExecutionState::Running
                    | crate::runtime::core::model::execution_graph::ExecutionState::Waiting
            )
        }) else {
            return Ok(TimeoutRound::Retire);
        };
        let branches = self.timeout_branches(snapshot, &execution.model_node)?;
        if branches.is_empty() {
            return Ok(TimeoutRound::Retire);
        }
        let cost_ms = crate::runtime::core::plan::timeout::cost_ms(
            now_ms,
            snapshot
                .variables
                .get(&(
                    owner.clone(),
                    crate::runtime::core::plan::timeout::STARTED_VAR.to_string(),
                ))
                .map(String::as_str),
        );
        match crate::runtime::core::plan::timeout::candidate(snapshot, owner, &branches) {
            crate::runtime::core::plan::timeout::TimeoutCandidate::None => Ok(TimeoutRound::Retire),
            crate::runtime::core::plan::timeout::TimeoutCandidate::Blocked => {
                Ok(TimeoutRound::Idle)
            }
            crate::runtime::core::plan::timeout::TimeoutCandidate::Branch(index) => {
                let branch = branches[index].clone();
                let guard = match self.timeout_guard(snapshot, &branch.task_id)? {
                    None => TimeoutGuard::Fire,
                    Some(expr) => {
                        // The window is decided on the engine's own
                        // environment (the legacy branch task's `init`), with
                        // the frozen cost the legacy `on_timeout` wrote into
                        // the task data.
                        let predicate = self
                            .evaluate_branch_predicate(
                                snapshot,
                                owner,
                                &expr,
                                &[(
                                    crate::utils::consts::TASK_COST.to_string(),
                                    cost_ms.to_string(),
                                )],
                            )
                            .await?;
                        match predicate {
                            crate::runtime::core::plan::branch::PredicateValue::True => {
                                TimeoutGuard::Fire
                            }
                            // The window is not open: no claim, the round ends
                            // (the legacy branch skips without claiming, so a
                            // later round re-evaluates it).
                            crate::runtime::core::plan::branch::PredicateValue::False => {
                                return Ok(TimeoutRound::Idle);
                            }
                            // A guard that cannot be evaluated is the legacy
                            // branch's `init` error: the branch is admitted and
                            // fails on its first hop.
                            crate::runtime::core::plan::branch::PredicateValue::Null => {
                                TimeoutGuard::Fail(format!(
                                    "timeout step '{}' guard evaluated to null",
                                    branch.task_id
                                ))
                            }
                            crate::runtime::core::plan::branch::PredicateValue::Invalid {
                                message,
                            } => TimeoutGuard::Fail(message),
                        }
                    }
                };
                Ok(TimeoutRound::Dispatch {
                    branch,
                    guard,
                    cost_ms,
                    declared: branches.len(),
                })
            }
        }
    }

    /// The plan of one dispatch round: the due branch admitted `Entry` through
    /// the container machinery (its execution, the owner's frozen cost and the
    /// advanced one-shot marker in one atomic commit) plus the timer's own
    /// replacement intent, so a later window still fires.
    #[allow(clippy::too_many_arguments)]
    fn timeout_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: Option<&crate::runtime::core::commit::aggregate::ExecutionSnapshot>,
        claim: &OperationCommit,
        branch: crate::runtime::core::plan::timeout::TimeoutBranch,
        guard: TimeoutGuard,
        cost_ms: i64,
        declared: usize,
    ) -> Result<TransitionPlan> {
        execution
            .ok_or_else(|| ActError::Runtime(format!("execution {execution_id:?} not found")))?;
        let mut pinned = Vec::new();
        if let TimeoutGuard::Fail(message) = guard {
            pinned.push((
                BRANCH_ERROR_VAR.to_string(),
                serde_json::json!({ "code": "", "message": message }).to_string(),
            ));
        }
        let claimed_after = crate::runtime::core::plan::timeout::claim(
            &crate::runtime::core::plan::timeout::claimed(snapshot, execution_id),
            &branch.task_id,
        );
        let dispatch = crate::runtime::core::plan::timeout::dispatch(
            snapshot,
            execution_id,
            claim.clone(),
            &branch,
            pinned,
            &claimed_after,
            cost_ms,
        );
        let mut plan = crate::runtime::core::plan::container::ContainerPlanner::plan_fanout(
            snapshot,
            &dispatch.command,
        )
        .map_err(runtime)?;
        if dispatch.claimed_after.len() < declared {
            // Declared branches remain unfired: the timer re-arms for them
            // instead of retiring (a window that has not opened yet must still
            // fire, and only a fresh round can see it).
            let sequence = next_plan_sequence(
                snapshot,
                &plan,
                &execution_id.task_id,
                execution_id.generation,
            );
            plan.operations.push(OperationIntent {
                id: OperationId {
                    process_id: id.process_id.clone(),
                    task_id: execution_id.task_id.clone(),
                    generation: execution_id.generation,
                    sequence,
                },
                kind: OperationKind::DispatchTimeout,
            });
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// The bare remainder of an operation with nothing to do: its own commit
    /// and nothing else. Completing the operation is the whole effect.
    fn empty_remainder(
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        claim: &OperationCommit,
    ) -> TransitionPlan {
        TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: Vec::new(),
        }
    }

    /// Whether an execution may not complete yet because a timeout branch it
    /// fired is still in flight. The legacy `auto_complete` counts every child
    /// task — the timeout branches included — so a step whose act is done waits
    /// for them; the kernel parks the execution in `Waiting` (a waiting act is
    /// already parked) and the container join finishes it.
    fn waits_for_timeouts(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: &crate::runtime::core::commit::aggregate::ExecutionSnapshot,
    ) -> Result<bool> {
        let branches = self.timeout_branches(snapshot, &execution.model_node)?;
        Ok(!branches.is_empty()
            && crate::runtime::core::plan::timeout::has_live_children(
                snapshot,
                execution_id,
                &branches,
            ))
    }

    /// The joined container's own completion message: a package-less
    /// container speaks here (its children settled and it reached its terminal
    /// state — the record is sequence-qualified, so a container that also
    /// announced an act keeps both). A `uses` step's act announced itself at
    /// the fan-out, so the join adds nothing for it: the legacy step's own
    /// messages are suppressed once it carries an act.
    fn attach_join_message(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        plan: &mut TransitionPlan,
        parent: &crate::runtime::core::model::execution_graph::ExecutionId,
        operation: &OperationIntent,
    ) -> Result<()> {
        let Some(execution) = snapshot.executions.get(parent) else {
            return Ok(());
        };
        if self.step_uses(snapshot, &execution.model_node)?.is_some() {
            return Ok(());
        }
        if let Some(payload) =
            self.task_message_payload(snapshot, parent, &parent.task_id, None, None)?
        {
            plan.outbox
                .push(crate::runtime::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "completed:{}:{}:{}:{}",
                        snapshot.process_id,
                        parent.task_id,
                        parent.generation,
                        operation.id.sequence
                    ),
                    topic: "task.completed".into(),
                    payload: payload.to_string(),
                });
        }
        Ok(())
    }

    /// The container fan-out plan: [`ContainerPlanner::plan_fanout`] plus the
    /// park mark the join reads.
    ///
    /// The mark belongs to this path, not to the planner: the container path is
    /// the one that parks an execution for the children it admitted, while a
    /// timeout dispatch admits a branch under an execution that is still
    /// waiting for its own act — marking that one would let the branch's
    /// settlement complete a step nobody answered (see
    /// [`crate::runtime::PARK_CHILDREN`]).
    fn container_fanout_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        command: &crate::runtime::core::plan::container::ContainerFanoutCommand,
    ) -> Result<TransitionPlan> {
        let mut plan =
            crate::runtime::core::plan::container::ContainerPlanner::plan_fanout(snapshot, command)
                .map_err(runtime)?;
        // A container without children completes in this very hop: it never
        // parks, so it carries no mark.
        if !command.children.is_empty() {
            mark_parked_for_children(&mut plan, &command.source);
        }
        // The container's own message, and only for a `uses` step: its act
        // completed here (the legacy act's task event fired before the step's
        // `auto_complete` saw the children). A package-less container is still
        // `Running` — the legacy step's emit rule skips that state — and its
        // task message belongs to the join that completes it.
        let source_node = snapshot
            .executions
            .get(&command.source)
            .map(|execution| execution.model_node.clone());
        let source_has_package = match &source_node {
            Some(node) => self.step_uses(snapshot, node)?.is_some(),
            None => false,
        };
        if source_has_package
            && let Some(payload) = self.hop_message_payload(
                snapshot,
                &command.source,
                &command.source.task_id,
                act_facts(snapshot, &command.source).as_ref(),
                Some(&plan.variable_writes),
                None,
            )?
        {
            plan.outbox
                .push(crate::runtime::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "completed:{}:{}:{}",
                        command.process_id, command.source.task_id, command.source.generation
                    ),
                    topic: "task.completed".into(),
                    payload: payload.to_string(),
                });
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Record that `execution` parked in `Waiting` for its children. The
    /// planner's own fact wins over a client input of the same private name.
    fn park_for_children_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: &crate::runtime::core::commit::aggregate::ExecutionSnapshot,
        claim: &OperationCommit,
        inputs: &[(String, String)],
    ) -> Result<TransitionPlan> {
        let mut plan = self.task_hop_plan(
            snapshot,
            id,
            execution_id,
            Some(execution),
            claim,
            "completed",
            "task.completed",
        )?;
        for (name, value) in inputs {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: name.clone(),
                value: value.clone(),
            });
        }
        // The act is answered, so the execution is no longer waiting for it:
        // it resumes `Running` while the branches it fired settle (the legacy
        // step's `next` after its act completed) — the state the join decides,
        // marked as parked.
        if execution.state == crate::runtime::core::model::execution_graph::ExecutionState::Waiting
        {
            plan.state_changes.push(StateChange {
                execution: execution_id.clone(),
                expected_revision: execution.revision,
                from: crate::runtime::core::model::execution_graph::ExecutionState::Waiting,
                to: crate::runtime::core::model::execution_graph::ExecutionState::Running,
            });
        }
        mark_parked_for_children(&mut plan, execution_id);
        // The act did complete: its message rides the record, exactly like a
        // leaf completion's. A package-less execution announces nothing here —
        // it is still running, and only its terminal decision is a message.
        if self.step_uses(snapshot, &execution.model_node)?.is_some()
            && let Some(payload) = self.hop_message_payload(
                snapshot,
                execution_id,
                &id.task_id,
                act_facts(snapshot, execution_id).as_ref(),
                Some(&plan.variable_writes),
                None,
            )?
        {
            plan.outbox[0].payload = payload.to_string();
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Fold one package outcome into the hop's plan: the returned outputs and
    /// the headless task's own data become the acting scope's writes, and the
    /// process env the package set merges into the run's durable env map on
    /// the root scope. All of it commits with the completion, so a crash
    /// before the commit re-runs the package against the same pre-state.
    fn fold_effect_outcome(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        outcome: &effects::EffectOutcome,
        plan: &mut TransitionPlan,
    ) {
        if let Some(outputs) = &outcome.outputs {
            for (name, value) in effects::outputs_as_writes(outputs) {
                plan.variable_writes.push(VariableWrite {
                    scope: execution_id.clone(),
                    name,
                    value,
                });
            }
        }
        for (name, value) in &outcome.data {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: name.clone(),
                value: value.to_string(),
            });
        }
        if !outcome.env.is_empty() {
            let Some(root) = root_execution(snapshot) else {
                return;
            };
            let mut map: serde_json::Map<String, serde_json::Value> = projection::env_map(snapshot)
                .unwrap_or_default()
                .into_iter()
                .collect();
            for (name, value) in &outcome.env {
                map.insert(name.clone(), value.clone());
            }
            plan.variable_writes.push(VariableWrite {
                scope: root,
                name: projection::ENV_MAP_VAR.to_string(),
                value: serde_json::Value::Object(map).to_string(),
            });
        }
    }

    /// The plan of an execution failure: the execution decided `Failed` with
    /// the error variables, its message on its own record, and the propagation
    /// that carries the error walk upward (the legacy `emit_error` path). An
    /// execution that already left the running states keeps its state — the
    /// bare hop remainder completes the operation. A step declaring `catches`
    /// never reaches the `Failed` decision: [`Self::catch_fanout`] keeps it
    /// running and dispatches its handlers instead.
    #[allow(clippy::too_many_arguments)]
    async fn failure_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: &crate::runtime::core::commit::aggregate::ExecutionSnapshot,
        claim: &OperationCommit,
        failure: &Failure,
    ) -> Result<TransitionPlan> {
        if let Some(plan) = self
            .catch_fanout(snapshot, claim, execution_id, execution, failure, &[])
            .await?
        {
            return Ok(plan);
        }
        let propagate_id = OperationId {
            process_id: id.process_id.clone(),
            task_id: id.task_id.clone(),
            generation: id.generation,
            sequence: id.sequence + 1,
        };
        let mut plan = self.task_hop_plan(
            snapshot,
            id,
            execution_id,
            Some(execution),
            claim,
            "failed",
            "task.failed",
        )?;
        if execution.state.is_terminal() {
            return Ok(plan);
        }
        plan.state_changes.push(StateChange {
            execution: execution_id.clone(),
            expected_revision: execution.revision,
            from: execution.state,
            to: crate::runtime::core::model::execution_graph::ExecutionState::Failed,
        });
        for (name, value) in [
            ("error.code", failure.code.clone()),
            ("error.message", failure.message.clone()),
        ] {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: name.to_string(),
                value,
            });
        }
        if !snapshot.operations.contains_key(&propagate_id) {
            plan.operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
            });
        }
        if let Some(payload) = self.hop_message_payload(
            snapshot,
            execution_id,
            &id.task_id,
            act_facts(snapshot, execution_id).as_ref(),
            None,
            Some((&failure.code, &failure.message)),
        )? {
            plan.outbox[0].payload = payload.to_string();
        }
        Ok(plan)
    }

    /// The plan of a package/params failure: the error the effect seam
    /// reports, under the kernel's package error code.
    async fn package_failure_plan(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: &crate::runtime::core::commit::aggregate::ExecutionSnapshot,
        claim: &OperationCommit,
        error: &crate::ActError,
    ) -> Result<TransitionPlan> {
        self.failure_plan(
            snapshot,
            id,
            execution_id,
            execution,
            claim,
            &package_failure(error),
        )
        .await
    }

    /// The plan of a caught failure: the legacy `Step::on_error`, which keeps
    /// a failed step `Running`, pins the error variables its CATCH predicates
    /// read (`$ecode()` answers the legacy `ecode`; the ERROR sign marks the
    /// step as deciding), and dispatches every `catches` child as a container
    /// child. The step's own join ([`crate::runtime::core::plan::container::ContainerPlanner::plan_catch_join`])
    /// decides it once they settled: every handler skipped resumes the error,
    /// anything else catches it.
    ///
    /// `None` when the decision is not a catchable one — the model node
    /// declares no `catches`, the execution is not running, or the step is
    /// already deciding a caught error (the legacy guard on
    /// `task.sign().is_none()`: a second failure skips the dispatch and walks
    /// the error up instead).
    #[allow(clippy::too_many_arguments)]
    async fn catch_fanout(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        claim: &OperationCommit,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        execution: &crate::runtime::core::commit::aggregate::ExecutionSnapshot,
        failure: &Failure,
        inputs: &[(String, String)],
    ) -> Result<Option<TransitionPlan>> {
        if !matches!(
            execution.state,
            crate::runtime::core::model::execution_graph::ExecutionState::Running
                | crate::runtime::core::model::execution_graph::ExecutionState::Waiting
        ) {
            return Ok(None);
        }
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, execution.model_node.as_str()) else {
            return Ok(None);
        };
        if step.catches.is_empty() || is_catch_decider(snapshot, execution_id) {
            return Ok(None);
        }
        // The error facts of the failed act, as the legacy task data carried
        // them: the handler's scope inherits them, so its guard and its params
        // select on `$ecode()` / `message` exactly like the legacy run. The
        // aggregate's variable representation is JSON text (a raw string would
        // not decode into the headless scope a predicate evaluates on), and
        // the failure — never the caller's copy — is what the act's data held.
        let mut overlay: Vec<(String, String)> = inputs
            .iter()
            .filter(|(name, _)| {
                !matches!(
                    name.as_str(),
                    "ecode" | "message" | "error.code" | "error.message"
                )
            })
            .cloned()
            .collect();
        overlay.push(("ecode".to_string(), json_var(&failure.ecode)));
        overlay.push(("message".to_string(), json_var(&failure.message)));
        let mut children = Vec::with_capacity(step.catches.len());
        for catch in &step.catches {
            let mut vars: Vec<(String, String)> = catch
                .vars
                .iter()
                .map(|var| (var.name.clone(), var.value.to_string()))
                .collect();
            // The legacy `TASK_SIGN = CATCH` the dispatch seeds into the
            // handler task: the catch join reads it back from the aggregate.
            vars.push((
                crate::runtime::core::plan::container::SIGN_VAR.to_string(),
                crate::runtime::core::plan::container::SIGN_CATCH.to_string(),
            ));
            let mut plan = crate::runtime::core::plan::container::ContainerChildPlan::Entry;
            if let Some(expr) = &catch.r#if {
                // The handler's own selection (the legacy `Step::init`
                // guard), evaluated with the error the step is deciding.
                match self
                    .evaluate_branch_predicate(snapshot, execution_id, expr, &overlay)
                    .await?
                {
                    PredicateValue::True => {}
                    PredicateValue::False => {
                        plan = crate::runtime::core::plan::container::ContainerChildPlan::Skip
                    }
                    PredicateValue::Null => vars.push((
                        BRANCH_ERROR_VAR.to_string(),
                        serde_json::json!({
                            "code": "",
                            "message": format!("catch step '{}' guard evaluated to null", catch.id)
                        })
                        .to_string(),
                    )),
                    PredicateValue::Invalid { message } => vars.push((
                        BRANCH_ERROR_VAR.to_string(),
                        serde_json::json!({ "code": "", "message": message }).to_string(),
                    )),
                }
            }
            children.push(crate::runtime::core::plan::container::ContainerChild {
                task_id: catch.id.clone(),
                model_node: node_id(&catch.id)?,
                plan,
                predecessors: vec![
                    crate::runtime::core::plan::container::ContainerPredecessor::Source,
                ],
                vars,
                auto: auto_decision(snapshot, &catch.id)?,
            });
        }
        // The step keeps running while its handlers work; the client's own
        // inputs land on its scope like any completion's — minus the error
        // names written explicitly below.
        let reserved = ["ecode", "message", "error.code", "error.message"];
        let inputs: Vec<(String, String)> = inputs
            .iter()
            .filter(|(name, _)| !reserved.contains(&name.as_str()))
            .cloned()
            .collect();
        let command = crate::runtime::core::plan::container::ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: execution_id.clone(),
            source_operation: claim.clone(),
            children,
            inputs,
            source_park: crate::runtime::core::model::execution_graph::ExecutionState::Running,
        };
        let mut plan = crate::runtime::core::plan::container::ContainerPlanner::plan_fanout(
            snapshot, &command,
        )
        .map_err(runtime)?;
        // The error facts of the failed act: the legacy task-data names a
        // catch predicate selects with (`$ecode()` / `message`) and the
        // kernel's own error variables, which the resumed error keeps
        // carrying up. Values are JSON text, the aggregate's representation.
        for (name, value) in [
            ("ecode".to_string(), json_var(&failure.ecode)),
            ("message".to_string(), json_var(&failure.message)),
            ("error.code".to_string(), json_var(&failure.code)),
            ("error.message".to_string(), json_var(&failure.message)),
            (
                // The deciding flow: durable plan state the catch join reads —
                // it is what tells a failed step apart from a container parked
                // for its children, both of which are `Running`. A raw engine
                // fact (`__`-private, never part of a predicate's scope).
                crate::runtime::core::plan::container::FLOW_VAR.to_string(),
                crate::runtime::core::plan::container::FLOW_CATCHES.to_string(),
            ),
        ] {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name,
                value,
            });
        }
        plan.validate().map_err(runtime)?;
        // The failed act's own message: the legacy act emitted `Error` before
        // the step recovered, so the client sees the failure the handler is
        // answering. Sequence-qualified, so the step's own later decision (a
        // resumed error) carries a record of its own.
        if let Some(payload) = self.hop_message_payload(
            snapshot,
            execution_id,
            &execution_id.task_id,
            act_facts(snapshot, execution_id).as_ref(),
            None,
            Some((&failure.code, &failure.message)),
        )? {
            plan.outbox
                .push(crate::runtime::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "failed:{}:{}:{}:{}",
                        snapshot.process_id,
                        execution_id.task_id,
                        execution_id.generation,
                        claim.id.sequence
                    ),
                    topic: "task.failed".into(),
                    payload: payload.to_string(),
                });
            plan.validate().map_err(runtime)?;
        }
        Ok(Some(plan))
    }

    /// Announce one execution's decided state on the planner's own outbox
    /// record: the legacy engine emitted a message per state transition — the
    /// act's for a `uses` step, the task's for a package-less one — so a client
    /// action that decides it (complete/submit/skip/cancel/abort/fail) must
    /// speak the same message. The record's topic already names the state; its
    /// opaque lifecycle payload becomes the hop's payload.
    fn attach_hop_message(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        plan: &mut TransitionPlan,
    ) -> Result<()> {
        let Some(change) = plan
            .state_changes
            .iter()
            .find(|change| &change.execution == execution_id)
        else {
            return Ok(());
        };
        if !snapshot.executions.contains_key(execution_id) {
            return Ok(());
        }
        let Some(topic) = message_topic(change.to) else {
            return Ok(());
        };
        // A `uses` step announces its act; a package-less execution announces
        // the task itself (`hop_message_payload` picks the shape).
        // The action's own writes carry the error variables a `Fail` action
        // supplies; later writes win, like the commit's own order.
        let read = |name: &str| {
            plan.variable_writes
                .iter()
                .rev()
                .find(|write| write.name == name)
                .map(|write| json_text(&write.value))
                .unwrap_or_default()
        };
        let error = (change.to
            == crate::runtime::core::model::execution_graph::ExecutionState::Failed)
            .then(|| (read("error.code"), read("error.message")));
        let error = error
            .as_ref()
            .map(|(code, message)| (code.as_str(), message.as_str()));
        let facts = act_facts(snapshot, execution_id);
        if let Some(payload) = self.hop_message_payload(
            snapshot,
            execution_id,
            &execution_id.task_id,
            facts.as_ref(),
            Some(&plan.variable_writes),
            error,
        )? {
            match plan.outbox.first_mut() {
                Some(record) => record.payload = payload.to_string(),
                None => {
                    plan.outbox
                        .push(crate::runtime::core::model::transition_plan::OutboxIntent {
                            key: format!(
                                "message:{}:{}:{}",
                                plan.process_id, execution_id.task_id, execution_id.generation
                            ),
                            topic: topic.to_string(),
                            payload: payload.to_string(),
                        })
                }
            }
        }
        Ok(())
    }

    /// The fan-out command for a container execution, or `None` when the
    /// model node is a plain leaf. Branch selection goes through the kernel's
    /// own `BranchPlanner::decide`; the inner chain of a branch is queued
    /// behind its first step.
    async fn fanout_command(
        &self,
        snapshot: &crate::runtime::core::commit::aggregate::AggregateSnapshot,
        execution_id: &crate::runtime::core::model::execution_graph::ExecutionId,
        claim: &OperationCommit,
        model_node: &crate::runtime::core::model::model_graph::NodeId,
        inputs: &[(String, String)],
    ) -> Result<Option<ContainerFanoutCommand>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(shape) = container_shape(&model, model_node) else {
            return Ok(None);
        };
        // A container whose children already left `Created` has fanned out
        // for this generation; a completion now is a force-finish (the
        // legacy client `Next` on a running step), not a second fan-out. A
        // rewound container's recreated children are all `Created` — exactly
        // like a never-fanned one — so they fan out (and readmit) again.
        //
        // Only the shape's own children count: a step that declares both
        // `branches` and `timeouts` has a live timeout branch under it as soon
        // as the tick fires one, and that branch must not read as "already
        // fanned out" — the legacy step schedules its branch children when it
        // runs into them, whatever a timeout did meanwhile.
        let declared: Vec<&str> = match &shape {
            ContainerShape::Branches(branches) => {
                branches.iter().map(|branch| branch.id.as_str()).collect()
            }
            ContainerShape::Steps(steps) => steps.iter().map(|step| step.id.as_str()).collect(),
        };
        if snapshot.executions.values().any(|execution| {
            execution.parent.as_ref() == Some(execution_id)
                && declared.contains(&execution.model_node.as_str())
                && execution.state
                    != crate::runtime::core::model::execution_graph::ExecutionState::Created
        }) {
            return Ok(None);
        }
        let children = match shape {
            ContainerShape::Branches(branches) => {
                let mut candidates = Vec::with_capacity(branches.len());
                for branch in branches {
                    candidates.push(BranchCandidate {
                        task_id: branch.id.clone(),
                        model_node: node_id(&branch.id)?,
                        predicate: match &branch.r#if {
                            Some(expr) => {
                                self.evaluate_branch_predicate(snapshot, execution_id, expr, inputs)
                                    .await?
                            }
                            None => {
                                if branch.r#else {
                                    PredicateValue::True
                                } else {
                                    PredicateValue::False
                                }
                            }
                        },
                        is_else: branch.r#else && branch.r#if.is_none(),
                    });
                }
                // A predicate that cannot be evaluated fails its branch — the
                // legacy branch task errors in its `init` — instead of wedging
                // the hop. Such a branch is never selected and never decides
                // anything for its siblings; in particular an `else` sibling
                // is blocked: the legacy else runs only when every `if`
                // sibling *skipped*, and an errored one did not.
                let branch_error = |task_id: &str| -> Option<String> {
                    candidates
                        .iter()
                        .find(|candidate| candidate.task_id == task_id)
                        .and_then(|candidate| match &candidate.predicate {
                            PredicateValue::Invalid { message } => Some(message.clone()),
                            _ => None,
                        })
                };
                let has_invalid = candidates
                    .iter()
                    .any(|candidate| branch_error(&candidate.task_id).is_some());
                let decidable: Vec<BranchCandidate> = candidates
                    .iter()
                    .filter(|candidate| {
                        branch_error(&candidate.task_id).is_none()
                            && !(has_invalid && candidate.is_else)
                    })
                    .cloned()
                    .collect();
                let selected: std::collections::HashSet<String> = if decidable.is_empty() {
                    Default::default()
                } else {
                    let command = BranchCommand {
                        process_id: snapshot.process_id.clone(),
                        branch: execution_id.clone(),
                        candidates: decidable,
                        predicate_basis: PredicateBasis::from_snapshot(snapshot),
                        unresolved_dependencies: Vec::new(),
                        source_operation: claim.clone(),
                    };
                    match BranchPlanner::decide(&command) {
                        BranchDecision::Selected(selected) => selected
                            .into_iter()
                            .map(|candidate| candidate.task_id)
                            .collect(),
                        BranchDecision::NoMatch => Default::default(),
                        BranchDecision::Waiting { dependencies } => {
                            return Err(runtime(format!(
                                "branch selection is waiting on unresolved {dependencies:?}"
                            )));
                        }
                        // Every candidate left in the command is evaluable, so
                        // an invalid decision here is a planner inconsistency.
                        BranchDecision::Invalid { message } => return Err(runtime(message)),
                    }
                };
                branches
                    .iter()
                    .map(|branch| {
                        let mut vars: Vec<(String, String)> = branch
                            .vars
                            .iter()
                            .map(|var| (var.name.clone(), var.value.to_string()))
                            .collect();
                        // The failing branch is admitted like a selected one:
                        // its first hop reads the pinned failure and decides
                        // this execution `Failed`, so the parity floor keeps
                        // its terminal execution (the legacy branch task).
                        if let Some(message) = branch_error(&branch.id) {
                            vars.push((
                                BRANCH_ERROR_VAR.to_string(),
                                serde_json::json!({ "code": "", "message": message }).to_string(),
                            ));
                        }
                        Ok(ContainerChild {
                            task_id: branch.id.clone(),
                            model_node: node_id(&branch.id)?,
                            // An unselected branch still becomes one terminal
                            // execution (Skipped) — the parity floor counts a
                            // terminal task per model node, and the legacy
                            // engine keeps the skipped branch task.
                            plan: if selected.contains(&branch.id)
                                || branch_error(&branch.id).is_some()
                            {
                                ContainerChildPlan::Entry
                            } else {
                                ContainerChildPlan::Skip
                            },
                            predecessors: vec![ContainerPredecessor::Source],
                            vars,
                            auto: true,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            ContainerShape::Steps(steps) => steps
                .iter()
                .enumerate()
                .map(|(index, step)| {
                    Ok(ContainerChild {
                        task_id: step.id.clone(),
                        model_node: node_id(&step.id)?,
                        plan: if index == 0 {
                            ContainerChildPlan::Entry
                        } else {
                            ContainerChildPlan::Queued
                        },
                        predecessors: if index == 0 {
                            vec![ContainerPredecessor::Source]
                        } else {
                            vec![ContainerPredecessor::Sibling(index - 1)]
                        },
                        vars: step
                            .vars
                            .iter()
                            .map(|var| (var.name.clone(), var.value.to_string()))
                            .collect(),
                        auto: auto_decision(snapshot, &step.id)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };
        Ok(Some(ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: execution_id.clone(),
            source_operation: claim.clone(),
            children,
            inputs: inputs.to_vec(),
            // A container that fanned out on its own completion stays
            // `Running` while its children run — the legacy step's state, which
            // is what `auto_complete` decides when the last child settled. A
            // source that was still `Waiting` (its act just answered) resumes
            // `Running` here, exactly like the legacy step's `next`.
            source_park: crate::runtime::core::model::execution_graph::ExecutionState::Running,
        }))
    }

    /// Claim → prepare one operation through atomic operation events.
    async fn operation_event(
        &self,
        id: &OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<(
        crate::runtime::core::model::operation::OperationRecord,
        crate::runtime::core::commit::aggregate::AggregateSnapshot,
    )> {
        match self
            .aggregates
            .commit_operation_event(id, expected_revision, event)
            .await
            .map_err(runtime)?
        {
            crate::runtime::durable::async_document_store::OperationCommitOutcome::Committed { operation, snapshot }
            | crate::runtime::durable::async_document_store::OperationCommitOutcome::AlreadyCommitted { operation, snapshot } => {
                Ok((operation, snapshot))
            }
            crate::runtime::durable::async_document_store::OperationCommitOutcome::Conflict { .. } => {
                Err(conflict(format!("claim conflict on operation {id:?}")))
            }
        }
    }

    async fn load(
        &self,
        process_id: &str,
    ) -> Result<Option<crate::runtime::durable::document_store::LoadedAggregate>> {
        self.aggregates.load(process_id).await.map_err(runtime)
    }

    pub(crate) async fn snapshot(
        &self,
        process_id: &str,
    ) -> Result<Option<crate::runtime::core::commit::aggregate::AggregateSnapshot>> {
        Ok(self.load(process_id).await?.map(|loaded| loaded.snapshot))
    }

    /// Whether the aggregate of `process_id` exists — the durable record of a
    /// pid. A start consults this before anything else so a restart of an
    /// existing run is refused (the legacy duplicate-pid rule), independently
    /// of the projected rows, which a swept run loses while its aggregate
    /// stays.
    pub(crate) async fn exists(&self, process_id: &str) -> Result<bool> {
        Ok(self.load(process_id).await?.is_some())
    }

    /// Whether every execution of the process reached a terminal state.
    #[cfg(test)]
    pub(crate) async fn process_completed(&self, process_id: &str) -> Result<bool> {
        Ok(self
            .snapshot(process_id)
            .await?
            .is_some_and(|snapshot| run_settled(&snapshot)))
    }

    /// Re-derive pending work from the recovery index — the single source of
    /// truth after a crash. Wake and reclaim dispositions re-enter the queue;
    /// committed-but-unfinalized operations are completed; effect-unknown
    /// operations are surfaced for manual reconciliation.
    pub(crate) async fn recover(&self, now_ms: i64) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let mut cursor = None;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        loop {
            let page = self
                .recovery
                .recoverable_page(cursor.as_ref(), PAGE_SIZE)
                .await
                .map_err(runtime)?;
            if page.operations.is_empty() {
                break;
            }
            for operation in page.operations {
                report.scanned += 1;
                match RecoveryPlanner::classify(&operation, now_ms) {
                    RecoveryDisposition::Wake => {
                        wake.wake(&operation.id);
                        report.woken.push(operation.id);
                    }
                    RecoveryDisposition::AwaitOwner => {
                        report.awaiting_owner.push(operation.id);
                    }
                    RecoveryDisposition::ReclaimExpired => {
                        match self
                            .operation_event(
                                &operation.id,
                                operation.revision,
                                OperationEvent::ReleaseExpiredClaim { now_ms },
                            )
                            .await
                        {
                            Ok((reclaimed, _)) => match reclaimed.state {
                                OperationState::RetryScheduled { .. } => {
                                    wake.wake(&reclaimed.id);
                                    report.woken.push(reclaimed.id);
                                }
                                OperationState::EffectUnknown { .. } => {
                                    report.reconciliation_required.push(reclaimed.id);
                                }
                                _ => {}
                            },
                            Err(error) => {
                                #[cfg(test)]
                                eprintln!("reclaim {operation:?} failed: {error}");
                                tracing::debug!(operation = ?operation.id, error = %error, "reclaim lost; next scan retries");
                            }
                        }
                    }
                    RecoveryDisposition::FinalizeCommit => {
                        let commit = match &operation.state {
                            OperationState::Committed(commit) => commit.clone(),
                            _ => continue,
                        };
                        match self
                            .operation_event(
                                &operation.id,
                                operation.revision,
                                OperationEvent::Complete { commit },
                            )
                            .await
                        {
                            Ok((finalized, _)) => report.finalized.push(finalized.id),
                            Err(error) => {
                                tracing::debug!(operation = ?operation.id, error = %error, "finalize lost; next scan retries");
                            }
                        }
                    }
                    RecoveryDisposition::ManualReconciliation => {
                        report.reconciliation_required.push(operation.id);
                    }
                    RecoveryDisposition::IgnoreTerminal => {}
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(report)
    }

    /// One outbox delivery pass over the attached transport: claim every due
    /// record with a fenced claim, hand it to the transport, then persist the
    /// reported outcome. After the transport ran, only its explicit `RetryAt`
    /// proves no external effect — everything else the transport cannot
    /// classify stays `DeliveryUnknown`. The transport lock is taken only for
    /// the synchronous `deliver` call, never across an await.
    pub(crate) async fn dispatch_outbox(&self) -> Result<usize> {
        let now = SystemClock.now_ms();
        let mut delivered = 0usize;
        let mut settled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cursor = None;
        loop {
            let page = self
                .aggregates
                .dispatchable_outbox_page(now, cursor.as_ref(), PAGE_SIZE)
                .await
                .map_err(runtime)?;
            if page.records.is_empty() {
                break;
            }
            for record in page.records {
                let claim = DeliveryClaim {
                    owner: OWNER.into(),
                    fence: record.last_fence + 1,
                    lease_until_ms: now + DELIVERY_LEASE_MS,
                    heartbeat_revision: 0,
                };
                let claimed = match self
                    .aggregates
                    .evolve_outbox(
                        &record.id,
                        record.revision,
                        OutboxEvent::Claim {
                            claim: claim.clone(),
                            now_ms: now,
                        },
                    )
                    .await
                {
                    Ok(claimed) => claimed,
                    Err(error) => {
                        // A concurrent worker claimed it first; skip without
                        // failing the pass.
                        tracing::debug!(record = ?record.id, error = %error, "delivery claim lost");
                        continue;
                    }
                };
                let event = {
                    let outcome = self.transport.write().deliver(&claimed);
                    match outcome {
                        crate::runtime::core::outbox::outbox_dispatch::DeliveryOutcome::Delivered { receipt } => {
                            delivered += 1;
                            settled.insert(claimed.id.process_id.clone());
                            OutboxEvent::ConfirmDelivered {
                                claim: claim.clone(),
                                receipt,
                                now_ms: now,
                            }
                        }
                        crate::runtime::core::outbox::outbox_dispatch::DeliveryOutcome::RetryAt {
                            available_at_ms,
                            error,
                        } => OutboxEvent::Reschedule {
                            claim: claim.clone(),
                            available_at_ms,
                            error,
                            now_ms: now,
                        },
                        crate::runtime::core::outbox::outbox_dispatch::DeliveryOutcome::OutcomeUnknown { reason } => {
                            OutboxEvent::MarkDeliveryUnknown {
                                claim: claim.clone(),
                                reason,
                                now_ms: now,
                            }
                        }
                        crate::runtime::core::outbox::outbox_dispatch::DeliveryOutcome::Quarantine { reason } => {
                            OutboxEvent::Quarantine { reason }
                        }
                    }
                };
                self.aggregates
                    .evolve_outbox(&record.id, claimed.revision, event)
                    .await
                    .map_err(runtime)?;
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        // The deliveries that settled decide the read model's `removable`
        // flag: a run that finished before its records were delivered only
        // hands its rows (and directory) to the sweeper once they are.
        for process_id in &settled {
            self.refresh_proc_row(process_id).await;
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests;
