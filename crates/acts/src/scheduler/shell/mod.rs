//! The shell: the async half of the runtime, driving the pure `core` over the
//! `durable` byte layer.
//!
//! [`Runtime`](crate::scheduler::Runtime) above owns it — it is the entry point
//! and the composition root; this module is what that entry point *runs*.
//!
//! The shell owns no state of its own — every fact lives in the aggregate
//! documents over the engine's [`KvStore`]. Normal-path liveness is the durable
//! ready index, written in the same atomic batch as the operation it describes;
//! a wake is only its accelerator. Losing a wake is safe — the driver's due pass
//! reads the same record — and `recover` handles only exceptional work (expired
//! claims, commits to finalize, effect-unknown reconciliation, missing ready
//! records). It takes each durable operation from claim to commit, one
//! operation kind per drive:
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
//! Every drive is one `commit_transition` with the operation it drives as the
//! plan's `source_operation`, so the operation is committed *and closed* inside the
//! same atomic batch as its business effect (the `Committed` → `Completed`
//! pair is applied in order there). When an operation's durable remainder
//! already exists (a wake redelivered after the commit), the drive commits an
//! empty plan sourced by its own operation, which completes the operation
//! without touching business state.
//!
//! The drive *pipeline* — the four phases one operation goes through, and the one
//! module per kind that implements the kind-specific two of them — is [`operation`];
//! this module is the host around it: the store and delivery seams, the
//! driver's slot bookkeeping, recovery, the outbox pass, the read model and
//! the plan vocabulary every kind plans with.

use crate::{
    ActError, NodeKind, Result,
    scheduler::{
        core::{
            commit::{
                aggregate::{ProcessSnapshot, RecoveryCandidate, RecoveryCursor, TaskSnapshot},
                command::{ActionCommand, ActionKind, ActionPlanner},
            },
            decision::{
                self, AdmissionCategory, AdmissionDirective, Decision, LifecycleDirective,
                QuotaDirective, RunFacts, SlotDirective, SlotObservation, SlotStanding,
            },
            model::{
                clock::{Clock, SystemClock},
                effect::EffectKey,
                model_graph::NodeId,
                operation::{
                    ClaimToken, OperationEvent, OperationId, OperationKind, OperationState,
                },
                task_graph::TaskId,
                transition_plan::{
                    OperationCommit, OperationIntent, StateChange, TransitionPlan, VariableWrite,
                },
            },
            outbox::{
                outbox_dispatch::DeliveryOutcome,
                outbox_repository::OutboxCursor,
                records::{DeliveryClaim, OutboxEvent, OutboxRecord},
            },
            plan::{
                ActPlan,
                branch::{
                    BranchCandidate, BranchCommand, BranchDecision, Decide, PredicateBasis,
                    PredicateValue,
                },
                concurrency::{self, Admission, Admit},
                container::{
                    CatchJoin, ContainerChild, ContainerChildPlan, ContainerFanoutCommand,
                    ContainerPredecessor, Fanout, Join,
                },
                generation::Reenter,
                infallible,
                propagation::{Propagate, PropagateCommand},
                timeout::{Dispatch, DispatchCommand, Round, RoundCommand},
                upward::Upward,
            },
            recovery::planner::{
                EffectReceipt, Observed, RecoveryDirective, RecoveryReport, recover_operation,
            },
        },
        durable::async_document_store::{
            AsyncProcessRepository, AsyncRecoveryRepository, KvDocumentStore, ReclaimOutcome,
            TransitionCommitOutcome,
        },
    },
    store::KvStore,
};

pub(crate) use crate::scheduler::core::health::{
    CleanupMetrics, KernelSchedule, LoopGuard, LoopHealth, OutboxMetrics, ReadyMetrics,
    RecoveryMetrics, SchedulerHealth, TimerGuards,
};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender, channel};

// The shell's parts are its own submodules: they see this module's scope and
// its private items, and this module re-exports the ones the runtime names
// through it.
mod action;
pub(crate) mod admission;
mod context;
pub(crate) mod delivery;
mod effects;
mod errors;
mod facts;
pub(crate) mod graph;
pub(crate) mod lifecycle;
mod liveness;
mod message;
mod model;
mod operation;
mod planning;
mod predicate;
mod projection;
mod read_model;
pub(crate) mod recovery;
pub(crate) mod scope;
pub(crate) mod snapshot;
pub(crate) mod timers;
pub(crate) mod validation;

use self::model::task_graph;
use self::{action::*, errors::*, facts::*, planning::*};
pub(crate) use admission::{AdmissionCaps, Admissions};
pub use context::Context;
pub(crate) use delivery::ShadowTransport;
pub(crate) use effects::PackageEffectRunner;
pub(crate) use lifecycle::ReportedRuns;
pub(crate) use lifecycle::{LifecycleSink, ProcessEvent};
pub(crate) use message::MessageForwarder;
pub(crate) use model::ProcessSpec;
pub(crate) use predicate::EnginePredicateRunner;
pub(crate) use projection::{ProjectionMetrics, RowProjector};
pub(crate) use read_model::{ProcRead, load as load_proc_rows};
#[cfg(test)]
pub(crate) use read_model::{TaskRow, context as read_context};
pub(crate) use scope::{Scope, ScopeTask};
#[cfg(test)]
pub(crate) use scope::{ScopeSpec, headless_scope};
pub(crate) use snapshot::SnapshotRegistry;
pub(crate) use timers::TimerGuider;

const OWNER: &str = "kernel-host";
const LEASE_MS: i64 = 60_000;
/// How many times a client action re-plans after losing a compare-and-swap.
/// The per-pid command lane (A4) serializes every in-process writer of one
/// run, so the normal path never exhausts — or starts — this loop; it stays as
/// the net for an out-of-lane writer (a guarded cleanup batch, another
/// instance) that moved the aggregate under the attempt. A retry re-reads the
/// snapshot; the action then either applies or is legitimately refused.
const ACTION_ATTEMPTS: usize = 8;
const DELIVERY_LEASE_MS: i64 = 30_000;
const PAGE_SIZE: usize = 128;

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
    processes: Arc<AsyncProcessRepository>,
    recovery: Arc<AsyncRecoveryRepository>,
    wake_tx: Sender<OperationId>,
    /// The effect seam: when attached, a Func package's work runs inside the
    /// ApplyAction operation and its outputs commit with the completion. Without
    /// one the host auto-completes (the shadow contract).
    effect: Arc<parking_lot::RwLock<Option<Arc<dyn effects::EffectRunner>>>>,
    /// The outbox delivery seam: the shadow logs; the driver's transport
    /// maps records onto engine messages (see `message::message_from_record`).
    transport: Arc<
        parking_lot::RwLock<
            Box<dyn crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport>,
        >,
    >,
    /// Retry delivery: a normal-path confirmation that lost its guard, or a
    /// record the transport deferred, notifies the driver so its outbox pass
    /// redelivers at once instead of waiting for the next tick. The tick stays
    /// the retry net.
    deliver: Arc<tokio::sync::Notify>,
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
    /// The runtime's store-facing timers (schedule triggers, delivery retries,
    /// terminal cleanup, snapshot purge), once attached: the driver's serve
    /// loop ticks them, so ONE background task owns every store-reaching pass.
    /// Attached after construction — the guider needs the runtime handle,
    /// which needs the host.
    timer_guider: Arc<parking_lot::RwLock<Option<Arc<TimerGuider>>>>,
    /// The timers' failure state and cleanup counters (see [`TimerGuards`]):
    /// the host owns them so `scheduler_health` answers every loop — the four
    /// driver passes and the four timers — from one place.
    timer_guards: Arc<TimerGuards>,
    /// Which runs were already reported: a start once, a terminal once.
    reported: Arc<parking_lot::Mutex<ReportedRuns>>,
    /// How often one node may run in one run — the legacy
    /// `max_node_run_times` bound, which the kernel keeps as the generation
    /// count of the node (every re-entry mints one). A flow that loops without
    /// a guard that closes is failed at the bound, exactly where the legacy
    /// `Process::create_task` errored the process.
    max_node_run_times: u64,
    /// The slot policy (`crate::scheduler::core::plan::concurrency`): the runs
    /// in flight at once, and the parked ones waiting for a slot. `start_process`
    /// reserves here and the driver settles here, so one lock owns the answer
    /// to "may this run operation".
    admissions: Arc<parking_lot::Mutex<Admissions>>,
    /// The tick cadence. A timer's ready record is deferred by one interval —
    /// the legacy engine has no timer round before its first process tick.
    timer_poll_ms: i64,
    /// The durable ready index changed (or a wake was lost): run a due pass
    /// now instead of waiting for the tick.
    ready: Arc<tokio::sync::Notify>,
    /// The first recovery pass repairs derived indexes from the aggregates.
    boot_reconciled: Arc<std::sync::atomic::AtomicBool>,
    /// The read model's boot catch-up completed. A failed scan (the store
    /// could not list the aggregates) is retried on the next recovery pass;
    /// per-pid projection failures are queued and retried independently.
    projection_booted: Arc<std::sync::atomic::AtomicBool>,
    ready_metrics: Arc<parking_lot::Mutex<ReadyMetrics>>,
    /// A10: each control-plane loop reports its own health and backlog.
    ready_health: Arc<LoopGuard>,
    recovery_health: Arc<LoopGuard>,
    outbox_health: Arc<LoopGuard>,
    projection_health: Arc<LoopGuard>,
    outbox_metrics: Arc<parking_lot::Mutex<OutboxMetrics>>,
    recovery_metrics: Arc<parking_lot::Mutex<RecoveryMetrics>>,
    /// The recovery scan's resume point across bounded passes (A10). The
    /// in-memory cursor is an optimization, not a durable fact: losing it
    /// (restart) only restarts the scan from the beginning.
    recovery_cursor: Arc<parking_lot::Mutex<Option<RecoveryCursor>>>,
    /// The outbox retry pass's resume point across bounded passes (A10).
    outbox_cursor: Arc<parking_lot::Mutex<Option<OutboxCursor>>>,
    /// One async lock per pid shared by a start and the terminal cleanup. The
    /// aggregate is the pid's durable existence record: a start checks it and
    /// creates the run under this lock, and cleanup holds it from the
    /// aggregate batch through the row/workdir removal. The window between
    /// "aggregate gone" and "derived rows gone" is therefore never visible to
    /// a start of the same pid.
    lifecycle_locks:
        Arc<parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// The per-pid command lane (A4): one single-writer turn per process for
    /// every command that decides state — the driver's operations, a client
    /// action's intent→claim→plan→commit, and recovery's mutating
    /// dispositions. Distinct pids have distinct lanes and stay parallel, and
    /// the lane is never held across an external effect (see
    /// [`operation::CommandLane`]). Revision guards remain the fence for what
    /// the lane cannot see: crash recovery, lease takeover and cross-instance
    /// writers.
    command_lanes:
        Arc<parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// How many times a client action re-planned after losing a
    /// compare-and-swap (A4). The command lane serializes the normal path, so
    /// a non-zero value means an out-of-lane writer (a guarded cleanup batch,
    /// another instance) moved the aggregate under a client — the retries the
    /// loop remains as a net for.
    action_replans: Arc<std::sync::atomic::AtomicU64>,
    /// Pids whose authoritative batch succeeded but whose row/workdir cleanup
    /// has not: a start must wait for the retry even though the aggregate is
    /// already gone. Cleared when the residue is removed.
    reclaiming: Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
    /// One-shot wakeups for durable run handles (`RunHandle::wait`): a pid's
    /// entry is taken and notified when a commit settles the run, or when
    /// reclamation removes its durable record. The wake is only an
    /// accelerator — a waiter always re-reads the aggregate, which is the
    /// truth — so a missed notification costs latency, never correctness.
    settled: Arc<parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Notify>>>>,
}

impl KernelHost {
    /// A host over the engine's key-value store, plus the wake receiver its
    /// driver task must own. `cache_cap` bounds the runs that may be in flight
    /// at once (see [`Admissions`]); tests pass
    /// `Arc::new(MemoryStore::new())` and a cap.
    #[cfg(test)]
    pub(crate) fn new(
        kv: Arc<dyn KvStore>,
        cache_cap: usize,
        max_node_run_times: u64,
        timer_poll_ms: i64,
    ) -> (Arc<Self>, Receiver<OperationId>) {
        Self::new_with_caps(
            kv,
            AdmissionCaps::new(cache_cap),
            max_node_run_times,
            timer_poll_ms,
        )
    }

    /// The host with explicit caps for every admission category (A8). The
    /// serving runtime builds these from [`Config`](crate::Config); tests that
    /// only exercise the execution bound keep [`Self::new`].
    pub(crate) fn new_with_caps(
        kv: Arc<dyn KvStore>,
        caps: AdmissionCaps,
        max_node_run_times: u64,
        timer_poll_ms: i64,
    ) -> (Arc<Self>, Receiver<OperationId>) {
        let store = Arc::new(KvDocumentStore::new(kv));
        let processes = Arc::new(AsyncProcessRepository::new(store));
        let recovery = Arc::new(AsyncRecoveryRepository::new(processes.clone()));
        let (wake_tx, wake_rx) = channel(PAGE_SIZE * 8);
        (
            Arc::new(Self {
                processes,
                recovery,
                wake_tx,
                effect: Arc::new(parking_lot::RwLock::new(None)),
                transport: Arc::new(parking_lot::RwLock::new(Box::new(ShadowTransport))),
                deliver: Arc::new(tokio::sync::Notify::new()),
                lifecycle: Arc::new(parking_lot::RwLock::new(None)),
                projection: Arc::new(parking_lot::RwLock::new(None)),
                predicate: Arc::new(parking_lot::RwLock::new(None)),
                timer_guider: Arc::new(parking_lot::RwLock::new(None)),
                timer_guards: Arc::new(TimerGuards::new()),
                reported: Arc::new(parking_lot::Mutex::new(ReportedRuns::default())),
                max_node_run_times,
                admissions: Arc::new(parking_lot::Mutex::new(Admissions::new(caps))),
                timer_poll_ms: timer_poll_ms.max(1),
                ready: Arc::new(tokio::sync::Notify::new()),
                boot_reconciled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                projection_booted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                ready_metrics: Arc::new(parking_lot::Mutex::new(ReadyMetrics::default())),
                ready_health: LoopGuard::new("ready-due"),
                recovery_health: LoopGuard::new("recovery-scan"),
                outbox_health: LoopGuard::new("outbox-retry"),
                projection_health: LoopGuard::new("projection-repair"),
                outbox_metrics: Arc::new(parking_lot::Mutex::new(OutboxMetrics::default())),
                recovery_metrics: Arc::new(parking_lot::Mutex::new(RecoveryMetrics::default())),
                recovery_cursor: Arc::new(parking_lot::Mutex::new(None)),
                outbox_cursor: Arc::new(parking_lot::Mutex::new(None)),
                lifecycle_locks: Arc::new(
                    parking_lot::Mutex::new(std::collections::HashMap::new()),
                ),
                command_lanes: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
                action_replans: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                reclaiming: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
                settled: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            }),
            wake_rx,
        )
    }

    /// The ready/timer pass's failure state (A10).
    pub(crate) fn ready_health(&self) -> LoopHealth {
        self.ready_health.snapshot()
    }

    /// The recovery scanner's failure state (A10).
    pub(crate) fn recovery_health(&self) -> LoopHealth {
        self.recovery_health.snapshot()
    }

    /// The outbox retry pass's failure state (A10).
    pub(crate) fn outbox_health(&self) -> LoopHealth {
        self.outbox_health.snapshot()
    }

    /// The projection repair pass's failure state (A10).
    pub(crate) fn projection_health(&self) -> LoopHealth {
        self.projection_health.snapshot()
    }

    /// The outbox retry pass's backlog/age gauges (A10).
    pub(crate) fn outbox_metrics(&self) -> OutboxMetrics {
        *self.outbox_metrics.lock()
    }

    /// The recovery scanner's backlog/counters (A10).
    pub(crate) fn recovery_metrics(&self) -> RecoveryMetrics {
        *self.recovery_metrics.lock()
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
        Self::new(
            Arc::new(crate::store::MemoryStore::new()),
            cache_cap,
            crate::Config::default().max_node_run_times() as u64,
            800,
        )
    }

    pub(crate) fn timer_poll_ms(&self) -> i64 {
        self.timer_poll_ms
    }

    /// Attach the effect runner: from then on a Func package's work runs
    /// inside the ApplyAction operation (see [`effects`]). Idempotent — the last
    /// attach wins; attach before driving operations.
    pub(crate) fn attach_effect_runner(&self, runner: Arc<dyn effects::EffectRunner>) {
        *self.effect.write() = Some(runner);
    }

    /// Replace the outbox transport: the shadow's logs records; the driver's
    /// transport maps them onto engine messages (see [`message`]). Attach
    /// before driving operations.
    pub(crate) fn attach_outbox_transport(
        &self,
        transport: Box<dyn crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport>,
    ) {
        *self.transport.write() = transport;
    }

    /// Attach the process-lifecycle sink: from then on a run reports one
    /// start when it is alive and one terminal event when every execution
    /// settled — the driver's `on_proc`. Attach before driving operations.
    pub(crate) fn attach_lifecycle_sink(&self, sink: LifecycleSink) {
        *self.lifecycle.write() = Some(sink);
    }

    /// Attach the read-model projector: from then on every commit
    /// re-materializes the legacy rows from the committed snapshot (see
    /// [`projection`]). Attach before driving operations.
    pub(crate) fn attach_row_projector(&self, projector: Arc<projection::RowProjector>) {
        *self.projection.write() = Some(projector);
    }

    /// Attach the predicate runner: from then on a branch `if` is decided on
    /// the engine's own environment on a headless task (see [`predicate`]);
    /// without one the built-in subset evaluator decides. Attach before
    /// driving operations.
    pub(crate) fn attach_predicate_runner(&self, runner: Arc<dyn predicate::PredicateRunner>) {
        *self.predicate.write() = Some(runner);
    }

    /// Attach the runtime's timer guider: the serve loop then ticks the four
    /// store-facing timers on their own intervals, so one task owns every
    /// background pass that reaches the store.
    pub(crate) fn attach_timers(&self, timers: Arc<TimerGuider>) {
        *self.timer_guider.write() = Some(timers);
    }

    /// The timers' guards and cleanup counters (the guider consults them, the
    /// health read reports them).
    pub(crate) fn timer_guards(&self) -> &Arc<TimerGuards> {
        &self.timer_guards
    }

    /// Every background loop's failure state: the four driver passes and the
    /// four store-facing timers, as one answer.
    pub(crate) fn scheduler_health(&self) -> SchedulerHealth {
        SchedulerHealth {
            trigger: self.timer_guards.trigger_health(),
            retry: self.timer_guards.retry_health(),
            cleanup: self.timer_guards.cleanup_health(),
            ready: self.ready_health(),
            recovery: self.recovery_health(),
            outbox: self.outbox_health(),
            projection: self.projection_health(),
        }
    }

    /// What the last terminal-cleanup passes observed and reclaimed.
    pub(crate) fn cleanup_metrics(&self) -> CleanupMetrics {
        self.timer_guards.cleanup_metrics()
    }

    /// Decide one branch `if`: on the engine's environment when a predicate
    /// runner is attached — a compile/eval failure of a *usable* expression
    /// included — otherwise on the built-in subset evaluator, which knows
    /// variables, operators and value methods but no engine function.
    async fn evaluate_branch_predicate(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        expr: &str,
        inputs: &[(String, String)],
    ) -> Result<PredicateValue> {
        let runner = self.predicate.read().clone();
        let (Some(runner), Some(model)) = (runner, model_of(snapshot)?) else {
            return Ok(evaluate_predicate(snapshot, task_id, expr, inputs));
        };
        match runner
            .evaluate(predicate::PredicateRequest {
                scope: self.scope_spec(snapshot, None, task_id, model)?,
                expr: expr.to_string(),
                overlay: inputs.to_vec(),
            })
            .await
        {
            Ok(value) => Ok(value),
            // A predicate the engine's environment cannot decide — an unset
            // variable, a non-boolean, a compile error — is the legacy branch
            // task's `init` error: the branch fails, the operation does not.
            Err(error) => Ok(PredicateValue::Invalid {
                message: format!("{error}"),
            }),
        }
    }

    /// Project one committed snapshot into the legacy rows, when a projector
    /// is attached. Returns whether the rows are now current: `true` also
    /// when no projector is attached (there is no read model to wait for).
    ///
    /// A store failure is reported and never propagated into the operation,
    /// but it is *not* ignored: the run enters the durable repair queue and
    /// the caller must not let an external observer (a lifecycle event, the
    /// outbox transport) see this commit first. `false` means exactly that —
    /// the repair pass will retry, and the outbox records stay `Pending` until
    /// it succeeds.
    async fn project(&self, snapshot: &ProcessSnapshot) -> bool {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return true;
        };
        let _projection = self.enter_admission(AdmissionCategory::Projection).await;
        if let Err(error) = projector.project(snapshot).await {
            // The run's last commit is also its last chance to derive the
            // rows without a later commit. Clear this run's claims and
            // re-derive the same version once (the durable revision guard
            // still refuses an older projection).
            if decision::run_settled(snapshot) {
                projector.forget_run(&snapshot.process_id);
                match projector.project_repair(snapshot).await {
                    Ok(_) => {
                        projector.finish_repair(&snapshot.process_id);
                        return true;
                    }
                    Err(retry) => tracing::warn!(
                        pid = %snapshot.process_id,
                        error = %retry,
                        "settled row projection repair failed; the repair queue retries it"
                    ),
                }
            } else {
                tracing::warn!(
                    pid = %snapshot.process_id,
                    error = %error,
                    "row projection failed; the repair queue will retry it"
                );
            }
            projector.enqueue_repair(&snapshot.process_id);
            return false;
        }
        projector.finish_repair(&snapshot.process_id);
        true
    }

    /// Project only the executions one commit touched (plus the proc row): the
    /// read model's rows for the rest of the run cannot have changed. Returns
    /// [`Self::project`]'s "rows are current" flag.
    async fn project_touched(
        &self,
        snapshot: &ProcessSnapshot,
        touched: &std::collections::BTreeSet<crate::scheduler::core::model::task_graph::TaskId>,
    ) -> bool {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return true;
        };
        let _projection = self.enter_admission(AdmissionCategory::Projection).await;
        if let Err(error) = projector.project_nodes(snapshot, Some(touched)).await {
            if decision::run_settled(snapshot) {
                projector.forget_run(&snapshot.process_id);
                match projector.project_repair(snapshot).await {
                    Ok(_) => {
                        projector.finish_repair(&snapshot.process_id);
                        return true;
                    }
                    Err(retry) => tracing::warn!(
                        pid = %snapshot.process_id,
                        error = %retry,
                        "settled row projection repair failed; the repair queue retries it"
                    ),
                }
            } else {
                tracing::warn!(
                    pid = %snapshot.process_id,
                    error = %error,
                    "row projection failed; the repair queue will retry it"
                );
            }
            projector.enqueue_repair(&snapshot.process_id);
            return false;
        }
        projector.finish_repair(&snapshot.process_id);
        true
    }

    /// The attached read model, if any.
    pub(crate) fn projector(&self) -> Option<Arc<projection::RowProjector>> {
        self.projection.read().clone()
    }

    /// The read model's counters and repair backlog.
    pub(crate) fn projection_metrics(&self) -> projection::ProjectionMetrics {
        self.projection
            .read()
            .as_ref()
            .map_or_else(Default::default, |projector| projector.metrics())
    }

    /// Project one run from its current aggregate as an explicit repair and
    /// report whether the rows are current. Returns `true` when the aggregate
    /// is gone (nothing to project).
    async fn repair_projection(&self, process_id: &str) -> bool {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return true;
        };
        let queued = projector.is_repair_pending(process_id);
        let _projection = self.enter_admission(AdmissionCategory::Projection).await;
        match self.load(process_id).await {
            Ok(Some(loaded)) => match projector.project_repair(&loaded.snapshot).await {
                Ok(_) => {
                    projector.finish_repair(process_id);
                    if queued {
                        projector.note_repaired();
                    }
                    // A commit whose projection failed withheld its lifecycle
                    // event (see `dispatch`); the repair is where the
                    // observer gets to see the now-projected state.
                    self.report_lifecycle(&loaded.snapshot).await;
                    // The same commit withheld its subflow hand-back; the
                    // repair creates it now (the helper is a no-op for a live
                    // or already-recorded child).
                    if let Err(error) = self.ensure_parent_return(&loaded.snapshot).await {
                        tracing::warn!(
                            pid = %process_id,
                            error = %error,
                            "recording the subflow parent hand-back after projection repair failed"
                        );
                    }
                    true
                }
                Err(error) => {
                    projector.enqueue_repair(process_id);
                    tracing::warn!(
                        pid = %process_id,
                        error = %error,
                        "row projection repair failed; the queue retries it"
                    );
                    false
                }
            },
            Ok(None) => {
                projector.finish_repair(process_id);
                true
            }
            Err(error) => {
                projector.enqueue_repair(process_id);
                tracing::warn!(
                    pid = %process_id,
                    error = %error,
                    "row projection repair failed; the queue retries it"
                );
                false
            }
        }
    }

    /// Retry one bounded batch of queued repairs. This is its own loop (A10),
    /// never part of the recovery scan: the queue is derived from the aggregate
    /// set, and a failed run stays queued until it projects.
    ///
    /// Returns how many repairs failed, which is the loop's health input.
    #[cfg(test)]
    pub(crate) async fn repair_projections(&self) -> usize {
        self.repair_projections_batch(32).await.1
    }

    /// Retry `batch` queued repairs, oldest first. Returns `(attempted,
    /// failed)`: a loop whose every attempt failed is the store failing, while
    /// one poisoned run among successful repairs must not degrade the loop.
    pub(crate) async fn repair_projections_batch(&self, batch: usize) -> (usize, usize) {
        let Some(projector) = self.projection.read().clone() else {
            return (0, 0);
        };
        let repairs = projector.pending_repairs(batch.max(1));
        let attempted = repairs.len();
        let mut failures = 0usize;
        for process_id in repairs {
            if !self.repair_projection(&process_id).await {
                failures += 1;
            }
        }
        (attempted, failures)
    }

    /// One boot pass: re-project every aggregate, so a database whose rows
    /// lagged (a crash between commit and projection, an older acts version)
    /// converges before the driver serves work.
    ///
    /// This is incremental by construction: every row carries its
    /// `source_revision`, and the conditional write skips rows already at the
    /// aggregate's version. The scan is linear in the aggregate set, once per
    /// host — never in the tick count.
    async fn catch_up_projections(&self) -> bool {
        let Some(projector) = self.projection.read().clone() else {
            return true;
        };
        let process_ids = match self.processes.process_ids().await {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(error = %error, "projection boot catch-up could not list aggregates");
                return false;
            }
        };
        let mut applied = 0usize;
        for process_id in process_ids {
            match self.load(&process_id).await {
                Ok(Some(loaded)) => {
                    match projector.project_repair(&loaded.snapshot).await {
                        Ok(report) => {
                            applied += report.applied;
                            if report.applied > 0 {
                                projector.note_repaired();
                            }
                            projector.finish_repair(&process_id);
                        }
                        Err(error) => {
                            projector.enqueue_repair(&process_id);
                            tracing::warn!(
                                pid = %process_id,
                                error = %error,
                                "projection boot catch-up failed; queued for repair"
                            );
                        }
                    }
                    // A child that settled before its return was recorded
                    // (the crash window between its terminal commit and the
                    // hand-back commit) gets its durable return now.
                    if let Err(error) = self.ensure_parent_return(&loaded.snapshot).await {
                        tracing::warn!(
                            pid = %process_id,
                            error = %error,
                            "recording a recovered subflow parent hand-back failed"
                        );
                    }
                }
                Ok(None) => {
                    projector.finish_repair(&process_id);
                }
                Err(error) => {
                    projector.enqueue_repair(&process_id);
                    tracing::warn!(
                        pid = %process_id,
                        error = %error,
                        "projection boot catch-up could not load an aggregate"
                    );
                }
            }
        }
        if applied > 0 {
            tracing::info!(
                runs = applied,
                "read model caught up from the durable aggregates"
            );
        }
        true
    }

    /// Take the pid's command lane (A4, see [`Self::command_lanes`]): the
    /// single-writer turn every state-deciding command of one process holds
    /// while it claims, plans and commits. The lane is per pid — distinct
    /// processes never queue behind each other — and no code path holds it
    /// across an external effect, a transport delivery or an executor
    /// observation. It is never nested with [`Self::lock_lifecycle`] or
    /// [`Self::lock_projection`]: cleanup takes those two and deliberately
    /// fences itself with the aggregate's revision guard instead of the lane.
    pub(crate) async fn lock_command_lane(
        &self,
        process_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let lane = {
            let mut lanes = self.command_lanes.lock();
            lanes
                .entry(process_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lane.lock_owned().await
    }

    /// How many client-action re-plans a lost compare-and-swap forced (A4).
    /// The lane keeps the normal path at zero; the counter is the metric that
    /// proves an out-of-lane writer, not the driver, moved the aggregate.
    pub(crate) fn action_replans(&self) -> u64 {
        self.action_replans
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Wait for and hold the pid's projection lock, when a read model is
    /// attached. Cleanup takes it before deleting the aggregate so an
    /// in-flight projection of the old run finishes first; the tombstone set
    /// by [`Self::reclaim_process`] then blocks every later one.
    pub(crate) async fn lock_projection(
        &self,
        process_id: &str,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let projector = self.projection.read().clone();
        match projector {
            Some(projector) => Some(projector.lock_process(process_id).await),
            None => None,
        }
    }

    /// Re-project one process from its durable aggregate (the sweeper's
    /// repair pass, and the test seam that re-derives rows a run's own
    /// commits did not). A run whose last operation committed before its
    /// records were delivered only becomes `removable` once they are, so the
    /// whole run is re-projected — tasks first, the proc row last — at the
    /// same aggregate version. Returns whether the rows are current.
    pub(crate) async fn refresh_projection(&self, process_id: &str) -> bool {
        self.repair_projection(process_id).await
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
                    projector.enqueue_repair(process_id);
                    tracing::warn!(
                        pid = %process_id,
                        error = %error,
                        "proc row projection failed; queued for repair"
                    );
                } else {
                    projector.finish_repair(process_id);
                }
            }
            Ok(None) => projector.finish_repair(process_id),
            Err(error) => {
                projector.enqueue_repair(process_id);
                tracing::warn!(
                    pid = %process_id,
                    error = %error,
                    "proc row projection refresh failed; queued for repair"
                );
            }
        }
    }

    /// The read model's boot catch-up: rows carry their source aggregate
    /// revision, so this projects every aggregate once and writes only the
    /// rows that lag. Owned by the projection loop; a per-pid failure is
    /// queued there, and a scan that could not list the aggregates is retried
    /// on its next pass.
    pub(crate) async fn ensure_projection_boot(&self) {
        if !self
            .projection_booted
            .load(std::sync::atomic::Ordering::SeqCst)
            && self.catch_up_projections().await
        {
            self.projection_booted
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The background driver (A10): executes woken operations as they arrive,
    /// and runs each control-plane loop on its own interval —
    /// [`KernelSchedule::ready`] for the durable ready queue and timers,
    /// [`KernelSchedule::recovery`] for the exceptional scan,
    /// [`KernelSchedule::outbox`] for delivery retries, and
    /// [`KernelSchedule::projection`] for row repair — plus the runtime's
    /// store-facing timers (schedule triggers, delivery retries, terminal
    /// cleanup, snapshot purge) once attached, on their own intervals. One
    /// task, one store-touching background service. Notifications still
    /// accelerate the ready and outbox passes; a lost wake is only latency,
    /// never lost work. Exits when `shutdown` fires.
    pub(crate) async fn serve(
        self: Arc<Self>,
        mut wake_rx: Receiver<OperationId>,
        shutdown: tokio_util::sync::CancellationToken,
        schedule: KernelSchedule,
    ) {
        // Boot repair is a one-time pass, not a loop: run it before the
        // tickers so a cold database has its derived indexes even when the
        // recovery interval is long. A failure is retried by the owning loop.
        if let Err(error) = self.ensure_boot(SystemClock.now_ms()).await {
            tracing::warn!(error = %error, "kernel boot repair failed; the ready loop retries it");
        }
        let mut ready_ticker = tokio::time::interval(schedule.ready);
        let mut recovery_ticker = tokio::time::interval(schedule.recovery);
        let mut outbox_ticker = tokio::time::interval(schedule.outbox);
        let mut projection_ticker = tokio::time::interval(schedule.projection);
        let mut trigger_ticker = tokio::time::interval(schedule.trigger);
        let mut retry_ticker = tokio::time::interval(schedule.retry);
        let mut cleanup_ticker = tokio::time::interval(schedule.cleanup);
        let mut snapshot_ticker = tokio::time::interval(schedule.snapshot);
        for ticker in [
            &mut ready_ticker,
            &mut recovery_ticker,
            &mut outbox_ticker,
            &mut projection_ticker,
            &mut trigger_ticker,
            &mut retry_ticker,
            &mut cleanup_ticker,
            &mut snapshot_ticker,
        ] {
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        }
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = self.ready.notified() => {
                    // A commit created ready work whose wake did not fit the
                    // channel, or the boot repair rebuilt the index: drive it
                    // now, not on the next tick.
                    let now = SystemClock.now_ms();
                    if let Err(error) = self.drive_ready(now, schedule.ready_budget).await {
                        tracing::warn!(error = %error, "kernel ready pass failed");
                    }
                }
                _ = self.deliver.notified() => {
                    // Bumped by a normal-path confirmation that lost its
                    // guard or by a transport that deferred a record: the
                    // pass runs while the failure is still fresh. A
                    // notification arriving while this pass runs is not lost —
                    // `Notify` keeps one permit, so the loop comes back for
                    // the next batch immediately instead of at the next tick.
                    if let Err(error) = self.dispatch_outbox_batch(schedule.outbox_budget).await {
                        tracing::warn!(error = %error, "kernel outbox pass failed");
                    }
                }
                _ = ready_ticker.tick() => {
                    // A degraded loop sits this tick out; ready work stays
                    // durable in the index and the next attempt picks it up.
                    if !self.ready_health.attempt() {
                        continue;
                    }
                    let now = SystemClock.now_ms();
                    let pass = async {
                        self.ensure_ready_boot(now).await?;
                        self.drive_ready(now, schedule.ready_budget).await
                    }
                    .await;
                    match pass {
                        Ok(_) => self.ready_health.recovered(),
                        Err(error) => {
                            tracing::warn!(error = %error, "kernel ready pass failed");
                            self.ready_health.failed(&error);
                        }
                    }
                    let metrics = self.ready_metrics();
                    if metrics.backlog > 0 {
                        tracing::debug!(
                            backlog = metrics.backlog,
                            due = metrics.due,
                            oldest_ready_age_ms = metrics.oldest_ready_age_ms,
                            deferred = metrics.deferred,
                            wake_failures = metrics.wake_failures,
                            repaired = metrics.repaired,
                            "kernel ready queue"
                        );
                    }
                    let admissions = self.admission_metrics();
                    if admissions.parked > 0
                        || admissions.quarantined_runs > 0
                        || admissions.reconciliation.active > 0
                    {
                        tracing::debug!(
                            execution_active = admissions.execution.active,
                            execution_cap = admissions.execution.cap,
                            parked = admissions.parked,
                            quarantined = admissions.quarantined_runs,
                            effect = admissions.effect.active,
                            effect_cap = admissions.effect.cap,
                            recovery = admissions.recovery.active,
                            reconciliation = admissions.reconciliation.active,
                            reconciliation_cap = admissions.reconciliation.cap,
                            projection = admissions.projection.active,
                            "kernel admission"
                        );
                    }
                }
                _ = recovery_ticker.tick() => {
                    if !self.recovery_health.attempt() {
                        continue;
                    }
                    let now = SystemClock.now_ms();
                    match self.recover_with_budget(now, schedule.recovery_budget).await {
                        Ok(report) => {
                            self.recovery_health.recovered();
                            if !report.reconciliation_required.is_empty() {
                                // An operation whose effect outcome could not
                                // be proven (no observation, a non-idempotent
                                // effect, or a failed replay) is never
                                // re-driven automatically (`ManualReconciliation`),
                                // so the run that owns it stops here. Nothing
                                // else reports it: without this line a
                                // stranded run is silent.
                                tracing::warn!(
                                    operations = ?report.reconciliation_required,
                                    "kernel recovery needs manual reconciliation for these operations; the runs that own them will not settle on their own"
                                );
                            }
                            if !report.effect_applied.is_empty()
                                || !report.effect_retried.is_empty()
                            {
                                tracing::info!(
                                    applied = ?report.effect_applied,
                                    retried = ?report.effect_retried,
                                    "kernel recovery reconciled unknown effects from executor observations"
                                );
                            }
                            let metrics = self.recovery_metrics();
                            if metrics.scanned > 0 || metrics.more_pending {
                                tracing::debug!(
                                    scanned = metrics.scanned,
                                    more_pending = metrics.more_pending,
                                    reclaimed = metrics.reclaimed,
                                    ready_repaired = metrics.ready_repaired,
                                    finalized = metrics.finalized,
                                    reconciliation_required = metrics.reconciliation_required,
                                    "kernel recovery scan"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "kernel recovery scan failed");
                            self.recovery_health.failed(&error);
                        }
                    }
                }
                _ = outbox_ticker.tick() => {
                    if !self.outbox_health.attempt() {
                        continue;
                    }
                    match self.dispatch_outbox_batch(schedule.outbox_budget).await {
                        Ok(_) => self.outbox_health.recovered(),
                        Err(error) => {
                            tracing::warn!(error = %error, "kernel outbox pass failed");
                            self.outbox_health.failed(&error);
                        }
                    }
                    let metrics = self.outbox_metrics();
                    if metrics.backlog > 0 {
                        tracing::debug!(
                            backlog = metrics.backlog,
                            observed = metrics.observed,
                            oldest_due_age_ms = metrics.oldest_due_age_ms,
                            delivered = metrics.delivered,
                            deferred = metrics.deferred,
                            "kernel outbox retry"
                        );
                    }
                }
                _ = projection_ticker.tick() => {
                    if !self.projection_health.attempt() {
                        continue;
                    }
                    self.ensure_projection_boot().await;
                    let (attempted, failures) =
                        self.repair_projections_batch(schedule.projection_budget).await;
                    if failures == 0 || failures < attempted {
                        // Some repairs succeeded: the store answers, and a
                        // single permanently-bad run must not degrade the loop.
                        self.projection_health.recovered();
                    } else {
                        let error = ActError::Runtime(format!(
                            "{failures}/{attempted} projection repairs failed"
                        ));
                        tracing::warn!(error = %error, "kernel projection repair pass failed");
                        self.projection_health.failed(&error);
                    }
                    let metrics = self.projection_metrics();
                    if metrics.lag > 0 {
                        tracing::debug!(
                            lag = metrics.lag,
                            oldest_lag_age_ms = metrics.oldest_lag_age_ms,
                            failed = metrics.failed,
                            repaired = metrics.repaired,
                            "kernel projection repair"
                        );
                    }
                }
                _ = trigger_ticker.tick() => {
                    let timers = self.timer_guider.read().clone();
                    if let Some(timers) = timers {
                        timers.trigger_tick().await;
                    }
                }
                _ = retry_ticker.tick() => {
                    let timers = self.timer_guider.read().clone();
                    if let Some(timers) = timers {
                        timers.retry_tick().await;
                    }
                }
                _ = cleanup_ticker.tick() => {
                    let timers = self.timer_guider.read().clone();
                    if let Some(timers) = timers {
                        timers.cleanup_tick().await;
                    }
                }
                _ = snapshot_ticker.tick() => {
                    let timers = self.timer_guider.read().clone();
                    if let Some(timers) = timers {
                        timers.purge_tick().await;
                    }
                }
                id = wake_rx.recv() => {
                    let Some(id) = id else { break };
                    // A panic anywhere while driving must not take the driver down
                    // with it: the engine would be left running with no owner
                    // for any operation. The operation's own user-code boundary (a
                    // package's `execute`) has already turned a package panic
                    // into an ordinary failure, so what reaches here is an
                    // engine panic — reported, and the operation is left open
                    // for recovery.
                    let run = match CatchPanic::new(self.execute_one(&id)).await {
                        Ok(run) => run,
                        Err(payload) => {
                            let message = panic_message(&payload);
                            tracing::error!(operation = ?id, panic = %message, "kernel operation panicked; left open for recovery");
                            continue;
                        }
                    };
                    if let Err(error) = run {
                        // The operation stays open; its durable ready record
                        // was removed by the claim, so recovery's lease expiry
                        // is the next owner.
                        #[cfg(test)]
                        eprintln!("operation {id:?} failed: {error}");
                        tracing::debug!(operation = ?id, error = %error, "kernel operation failed; left open for recovery");
                    }
                }
            }
        }
    }

    /// Drive the due records of the durable ready index, up to `budget`
    /// operations. This is normal-path liveness: the wake channel above is only
    /// the fast path.
    ///
    /// A record the driver cannot run now — a parked run, or a timer whose
    /// round is still idle — is deferred one poll interval; it never spins
    /// and never has to be recovered.
    pub(crate) async fn drive_ready(&self, now_ms: i64, budget: usize) -> Result<usize> {
        self.ready_metrics.lock().passes += 1;
        let mut executed = 0usize;
        let mut cursor = None;
        let mut round_executed = 0usize;
        let mut observed_peak = 0usize;
        let mut oldest: Option<i64> = None;
        loop {
            // A commit made by an operation in this pass stamps its successors
            // with the wall clock. Re-read it so a full wake queue does not
            // leave them waiting for the next tick; test callers that pass a
            // future instant still select it.
            let due_now = now_ms.max(SystemClock.now_ms());
            let page = self
                .processes
                .due_ready_page(cursor.as_ref(), due_now, PAGE_SIZE)
                .await
                .map_err(runtime)?;
            observed_peak = observed_peak.max(page.observed);
            oldest = match (oldest, page.oldest_ready_at_ms) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            };
            if page.operations.is_empty() {
                break;
            }
            let due = page.operations.len();
            {
                let mut metrics = self.ready_metrics.lock();
                metrics.backlog = observed_peak;
                metrics.due = due;
                metrics.oldest_ready_age_ms =
                    oldest.map(|ready_at| due_now.saturating_sub(ready_at).max(0));
                metrics.scanned += observed_peak as u64;
            }
            for id in page.operations {
                if executed >= budget {
                    self.ready.notify_one();
                    return Ok(executed);
                }
                match CatchPanic::new(self.execute_one(&id)).await {
                    Ok(Ok(true)) => {
                        executed += 1;
                        round_executed += 1;
                    }
                    Ok(Ok(false)) => {
                        // Nothing to do now: defer instead of spinning. The
                        // operation stays durable and the next poll re-checks
                        // it (a parked run's admission, an idle timer round).
                        let until = SystemClock.now_ms().saturating_add(self.timer_poll_ms);
                        if self
                            .processes
                            .set_ready_at(&id, until)
                            .await
                            .map_err(runtime)?
                        {
                            self.ready_metrics.lock().deferred += 1;
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::debug!(operation = ?id, error = %error, "ready operation failed; left open for recovery");
                    }
                    Err(payload) => {
                        let message = panic_message(&payload);
                        tracing::error!(operation = ?id, panic = %message, "ready operation panicked; left open for recovery");
                    }
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                // One walk reached the end of the index. Execution may have
                // inserted records behind the walk's start (new successors),
                // so a productive walk restarts once — the durable queue, not
                // the wake channel, must drain itself. A walk that ran nothing
                // means the remaining due rows could not be driven (a parked
                // run, an owner that moved); stop instead of spinning.
                if round_executed == 0 {
                    break;
                }
                round_executed = 0;
            }
        }
        Ok(executed)
    }

    /// Create the process atomically and wake its root operation — or park it,
    /// when every slot is taken (see [`Admissions`]).
    pub(crate) async fn start_process(&self, spec: &ProcessSpec) -> Result<String> {
        // A reclaimed pid is a new run: clear the projection tombstone the
        // cleanup left so the new run's rows are written.
        self.resume_process(&spec.process_id);
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
            .processes
            .create(&snapshot, SystemClock.now_ms(), self.timer_poll_ms)
            .await
            .map_err(repository_error)
        {
            // The reservation never became a run: the slot (or the parked
            // entry) must not outlive the failed creation.
            self.admissions.lock().abandon(&spec.process_id);
            return Err(error);
        }
        // The read model from the first durable fact: the proc row and the
        // seeded task rows exist before the root operation runs, so an immediately
        // following read sees the run. A parked run's proc row reads `None`,
        // exactly like the legacy parked row's.
        self.project(&snapshot).await;
        let root = OperationId {
            process_id: spec.process_id.clone(),
            task: crate::scheduler::core::model::task_graph::TaskId::new(&spec.root, 0)
                .map_err(runtime)?,
            sequence: 0,
        };
        match admission {
            Admission::Admit => {
                self.wake(&root);
            }
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

    pub(crate) fn ready_metrics(&self) -> ReadyMetrics {
        *self.ready_metrics.lock()
    }

    /// Drive woken operations until the queue drains or `max_steps` operations ran.
    /// Test-side twin of [`Self::serve`]: same operation execution, deterministic
    /// drain instead of a select loop.
    #[cfg(test)]
    pub(crate) async fn run_until_quiescent(
        &self,
        wake_rx: &mut Receiver<OperationId>,
        max_steps: usize,
    ) -> Result<RunReport> {
        let mut report = RunReport::default();
        let mut steps = 0usize;
        while steps < max_steps {
            let mut progressed = false;
            while steps < max_steps {
                let id = match wake_rx.try_recv() {
                    Ok(id) => id,
                    Err(_) => break,
                };
                steps += 1;
                progressed = true;
                match CatchPanic::new(self.execute_one(&id)).await {
                    Ok(Ok(true)) => report.executed += 1,
                    Ok(Ok(false)) => report.ignored += 1,
                    Ok(Err(error)) => {
                        report.retried += 1;
                        #[cfg(test)]
                        eprintln!("operation {id:?} failed: {error}");
                        tracing::debug!(operation = ?id, error = %error, "operation failed; operation stays open for recovery");
                    }
                    Err(payload) => {
                        report.retried += 1;
                        let message = panic_message(&payload);
                        #[cfg(test)]
                        eprintln!("operation {id:?} panicked: {message}");
                        tracing::error!(operation = ?id, panic = %message, "operation panicked; operation stays open for recovery");
                    }
                }
            }
            if steps < max_steps {
                let executed = self
                    .drive_ready(SystemClock.now_ms(), max_steps - steps)
                    .await?;
                steps += executed;
                report.executed += executed;
                progressed |= executed > 0;
            }
            if !progressed {
                break;
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
        node: &str,
        action: crate::scheduler::core::commit::command::ActionKind,
        inputs: Vec<(String, String)>,
    ) -> Result<()> {
        // The pid's command lane (A4) serializes this attempt against the
        // driver's operations and every other client of the same run, so the
        // normal path commits on the first attempt. The loop below stays as
        // the net for what the lane cannot see — an out-of-lane writer (a
        // guarded cleanup batch, another instance) moving the aggregate under
        // the attempt: a retry re-plans against the fresh snapshot, the action
        // either applies or is legitimately refused, and a refusal is never
        // retried. The retry pins the intent's identity (see [`ActionIntent`])
        // and consults it first: an attempt that committed its intent and then
        // lost the completion compare-and-swap must report success, never a
        // refusal caused by the state its own commit moved.
        let mut pinned: Option<ActionIntent> = None;
        let mut attempt = 0;
        loop {
            match self
                .apply_action_once(
                    process_id,
                    node,
                    action.clone(),
                    inputs.clone(),
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
                        match self.resume_action_intent(intent, &action, &inputs).await? {
                            ActionResume::Applied => return Ok(()),
                            ActionResume::Retry => {}
                        }
                    }
                    // Two clients no longer race on one revision (the lane
                    // serializes them), but the racing *decision* still
                    // exists: the winner's commit is what stale this
                    // attempt's guard. A short pause lets that decision
                    // settle; the next attempt plans against the state it
                    // left and answers with the refusal the loser actually
                    // earned.
                    self.action_replans
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    tracing::debug!(
                        pid = %process_id,
                        tid = %node,
                        attempt,
                        replans = self.action_replans(),
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
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        source: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<()> {
        let graph = task_graph(snapshot)?;
        let modelled = model_of(snapshot)?.is_some();
        let mut seen = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<
            crate::scheduler::core::model::task_graph::TaskId,
        > = std::collections::VecDeque::from([source.clone()]);
        let mut forward: Vec<&crate::scheduler::core::commit::aggregate::TaskSnapshot> = Vec::new();
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            for successor in graph.successors(&current) {
                let Some(task) = snapshot.tasks.get(successor) else {
                    continue;
                };
                // A child of `current` is the container's own work, not the
                // chain's next step — the legacy walk follows `next` tasks, so
                // children stay out of the guard. The rewind still recreates
                // them: they belong to the closure the planner rebuilds.
                if task.parent.as_ref() == Some(&current) {
                    continue;
                }
                let package_step = !modelled
                    || self
                        .step_uses(snapshot, successor, &task.model_node)?
                        .is_some();
                if package_step {
                    forward.push(task);
                }
                queue.push_back(successor.clone());
            }
        }
        if forward.is_empty() {
            return Err(ActError::Action("cannot find cancelled tasks".to_string()));
        }
        if forward.iter().all(|task| task.state.is_terminal()) {
            return Err(ActError::Action(format!(
                "task('{}') is not allowed to cancel",
                source.node.as_str()
            )));
        }
        Ok(())
    }

    /// The identity one action attempt decides under: the pinned one while it
    /// is still ours, otherwise a freshly minted one.
    ///
    /// The minted id is `max(sequence) + 1` over the execution's own
    /// operations, so two client actions that loaded the same state mint the
    /// *same* one. An operation already standing there belongs to another
    /// client's decision: the next free sequence is taken instead, so the
    /// attempt re-plans *our* decision — the preflight then decides whether it
    /// still applies to the state the other decision moved, or is legitimately
    /// refused — and never resumes an operation whose outcome another client
    /// was already told.
    fn action_identity(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        process_id: &str,
        node: &str,
        action: &crate::scheduler::core::commit::command::ActionKind,
        inputs: &[(String, String)],
        pinned: &mut Option<ActionIntent>,
    ) -> Result<ActionIntent> {
        if let Some(intent) = pinned.as_ref()
            && Self::action_intent_is_mine(snapshot, intent, action, inputs)
        {
            return Ok(ActionIntent {
                task: intent.task.clone(),
                operation: intent.operation.clone(),
            });
        }
        let task_id = Self::action_target(snapshot, process_id, node, action)?;
        let mut sequence = snapshot
            .operations
            .keys()
            .filter(|id| id.process_id == process_id && id.task == task_id)
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        loop {
            let candidate = ActionIntent {
                task: task_id.clone(),
                operation: OperationId {
                    process_id: process_id.into(),
                    task: task_id.clone(),
                    sequence,
                },
            };
            if !snapshot.operations.contains_key(&candidate.operation) {
                *pinned = Some(ActionIntent {
                    task: candidate.task.clone(),
                    operation: candidate.operation.clone(),
                });
                return Ok(candidate);
            }
            sequence += 1;
        }
    }

    /// The execution one client action on `task_id` names.
    ///
    /// The action surface is addressed by the `tid` a message carried — since
    /// C5 that is the instance's wire (`sha256(pid, node, gen)[..32]`), so a
    /// client answering one instance's message decides *that* instance, not
    /// whatever sibling happens to be the node's newest generation. The
    /// selection itself is untouched (§11 C4/C5): the newest generation that
    /// can accept the action wins, and with no generation that can take it,
    /// the newest is named anyway so the preflight answers the refusal the
    /// caller earned (a duplicate delivery, a decision on a settled node)
    /// rather than a missing execution. The legacy engine's per-item act
    /// carried a fresh id, and the kernel's pre-C5 node identity could not
    /// tell the instances apart.
    fn action_target(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        process_id: &str,
        node: &str,
        action: &crate::scheduler::core::commit::command::ActionKind,
    ) -> Result<crate::scheduler::core::model::task_graph::TaskId> {
        use crate::scheduler::core::commit::command::action_allowed;
        let generations = || {
            snapshot
                .tasks
                .keys()
                .filter(|id| id.wire(process_id) == node)
        };
        generations()
            .filter(|id| action_allowed(snapshot.tasks[*id].state, action))
            .max_by_key(|id| id.generation)
            .or_else(|| generations().max_by_key(|id| id.generation))
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("execution {node} not found")))
    }

    /// Whether the operation at `intent` carries *this* decision: it exists and
    /// the fact committed with it (the same batch) encodes the same action and
    /// inputs. An operation without that fact, or with another decision's, is
    /// not ours to resume.
    fn action_intent_is_mine(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        intent: &ActionIntent,
        action: &crate::scheduler::core::commit::command::ActionKind,
        inputs: &[(String, String)],
    ) -> bool {
        snapshot.operations.contains_key(&intent.operation)
            && snapshot
                .variables
                .get(&(
                    intent.task.clone(),
                    action_fact_var(intent.operation.sequence),
                ))
                .is_some_and(|fact| fact == &action_fact(action, inputs))
    }

    /// The durable intent of one client action: the `ApplyAction` operation
    /// that carries it, plus the action itself as the private fact its operation
    /// replays. Without that fact a recovery driving the operation (a crash,
    /// an expired lease) would plan the bare operation's auto completion instead of
    /// the decision the client made.
    fn action_intent(
        process_id: &str,
        task: &crate::scheduler::core::model::task_graph::TaskId,
        operation: &OperationId,
        action: &crate::scheduler::core::commit::command::ActionKind,
        inputs: &[(String, String)],
        expected: crate::scheduler::core::model::transition_plan::ProcessVersion,
    ) -> TransitionPlan {
        TransitionPlan {
            process_id: process_id.into(),
            expected,
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: vec![VariableWrite {
                scope: task.clone(),
                name: action_fact_var(operation.sequence),
                value: action_fact(action, inputs),
            }],
            source_operation: None,
            operations: vec![OperationIntent {
                id: operation.clone(),
                kind: OperationKind::ApplyAction,
                ready_at_ms: None,
            }],
            outbox: Vec::new(),
        }
    }

    /// Whether a retried action's pinned intent already carries its commit —
    /// the action applied, so the retry reports success — and the release of
    /// an intent whose effect never committed (the attempt lost its
    /// compare-and-swap mid-sequence), so the next attempt re-drives the same
    /// operation instead of stranding it.
    ///
    /// The pin is only consulted for *our* decision: an operation at the
    /// pinned id that carries another action's fact is a racer's, and `Retry`
    /// sends the caller to an attempt that drops the pin and re-mints (see
    /// [`Self::action_identity`]). Reporting `Applied` for it would hand this
    /// client the other one's outcome.
    async fn resume_action_intent(
        &self,
        intent: &ActionIntent,
        action: &crate::scheduler::core::commit::command::ActionKind,
        inputs: &[(String, String)],
    ) -> Result<ActionResume> {
        // A4: the release below is a command on the same pid — it takes the
        // lane; the operation revision it carries stays the fence.
        let _lane = self.lock_command_lane(&intent.operation.process_id).await;
        let Some(snapshot) = self.load(&intent.operation.process_id).await? else {
            return Ok(ActionResume::Retry);
        };
        if !Self::action_intent_is_mine(&snapshot.snapshot, intent, action, inputs) {
            return Ok(ActionResume::Retry);
        }
        let Some(operation) = snapshot.snapshot.operations.get(&intent.operation) else {
            return Ok(ActionResume::Retry);
        };
        match &operation.state {
            // The effect commit is durable, the rewind operation it created
            // included: recovery completes the bookkeeping and drives the
            // rewind, the client's action has applied.
            crate::scheduler::core::model::operation::OperationState::Committed(_)
            | crate::scheduler::core::model::operation::OperationState::Completed(_) => {
                Ok(ActionResume::Applied)
            }
            crate::scheduler::core::model::operation::OperationState::Claimed(claim)
            | crate::scheduler::core::model::operation::OperationState::CommitPrepared(claim) => {
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
            crate::scheduler::core::model::operation::OperationState::EffectUnknown { .. }
            | crate::scheduler::core::model::operation::OperationState::Quarantined { .. } => {
                Err(runtime(format!(
                    "action intent {:?} needs reconciliation: {:?}",
                    intent.operation, operation.state
                )))
            }
            _ => Ok(ActionResume::Retry),
        }
    }

    /// One action application: the preflight, the durable intent, and then the
    /// standard operation over the operation that intent names (see
    /// [`Self::apply_action`] for the retry around it). `pinned` carries the
    /// identity the first attempt fixed.
    async fn apply_action_once(
        &self,
        process_id: &str,
        node: &str,
        action: crate::scheduler::core::commit::command::ActionKind,
        inputs: Vec<(String, String)>,
        pinned: &mut Option<ActionIntent>,
    ) -> Result<()> {
        // A4: the whole attempt — load, preflight, durable intent, claim,
        // plan, commit — holds the pid's command lane, so the driver's
        // operation and every other client of the same run decide against the
        // state this attempt commits, never beside it on the same revision.
        // The effect phase releases the lane exactly like the driver's (see
        // `operation::CommandLane`).
        let mut lane = operation::CommandLane::hold(self, process_id).await;
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {process_id} not found")))?
            .snapshot;
        // The identity this attempt decides under. A pinned identity stands
        // only while it is still *ours*: the minted id is `max(sequence) + 1`,
        // so a concurrent client action can hold the same one, and resuming a
        // foreign operation would report the other client's outcome to this
        // one.
        let intent = Self::action_identity(&snapshot, process_id, node, &action, &inputs, pinned)?;
        let (task_id, operation_id) = (intent.task.clone(), intent.operation.clone());
        let now = SystemClock.now_ms();
        // The preflight envelope: `ActionPlanner` validates the whole command —
        // identity and sequence included — before the durable intent exists, so
        // a caller-correctable refusal consumes nothing. The claim on this
        // envelope is the envelope's own; the operation takes its claim in phase 1,
        // fenced by the operation's own attempt, which is what the state
        // machine checks.
        let command = ActionCommand {
            process_id: process_id.into(),
            task: task_id.clone(),
            action,
            inputs,
            source_operation: OperationCommit {
                id: operation_id.clone(),
                claim: ClaimToken {
                    owner: OWNER.into(),
                    fence: 1,
                    lease_until_ms: now + LEASE_MS,
                    heartbeat_revision: 0,
                },
            },
        };

        // Preflight against the current snapshot: a caller-correctable
        // rejection (terminal task, unknown execution) consumes nothing.
        ActionPlanner::plan(&snapshot, &command).map_err(runtime)?;
        // A `Back` without the step it names is the legacy refusal, checked
        // before the durable intent: the rewind target is the client's own
        // input, not something the kernel can invent.
        if matches!(command.action, ActionKind::Back)
            && !command
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
        if matches!(command.action, ActionKind::Cancel) {
            self.validate_cancel_forward_work(&snapshot, &task_id)?;
        }
        // A step declaring `catches` is not a client target once it is deciding
        // its caught error: the legacy act it folds is already terminal, and the
        // legacy `do_action` refuses every action on it. The durable flow fact
        // is exactly that state — the same operation that dispatched the handlers
        // pinned it — so no child-proxy is needed (a timeout branch the tick
        // fired is a child too, and the step's own act stays actionable while
        // one runs). The refusal is part of the preflight, so it consumes no
        // operation.
        if let Some(model) = model_of(&snapshot)?
            && let Some(node) = snapshot
                .tasks
                .get(&task_id)
                .map(|task| task.model_node.as_str())
            && let Some(step) = find_step_in_model(&model, node)
            && !step.catches.is_empty()
            && is_catch_decider(&snapshot, &task_id)
        {
            return Err(ActError::Action(format!(
                "step '{}' is deciding its catches; the act it folds is decided",
                task_id.node.as_str()
            )));
        }

        // Durable intent: the operation that carries the action *and* the
        // action itself, as a private fact its operation replays (see
        // `action_intent`). Written once per decision, not once per attempt: a
        // retry whose intent already landed goes straight to the claim, and an
        // operation already standing at the id (a racer's, or our own from a
        // released attempt) is a retryable race, never a domain error.
        if !Self::action_intent_is_mine(&snapshot, &intent, &command.action, &command.inputs) {
            let plan = Self::action_intent(
                process_id,
                &task_id,
                &operation_id,
                &command.action,
                &command.inputs,
                snapshot.version.clone(),
            );
            match self
                .processes
                .commit_transition(&plan, SystemClock.now_ms(), self.timer_poll_ms)
                .await
                .map_err(repository_error)?
            {
                TransitionCommitOutcome::Committed { .. }
                | TransitionCommitOutcome::AlreadyCommitted { .. } => {}
                TransitionCommitOutcome::Conflict { .. } => {
                    return Err(conflict(format!(
                        "action intent conflict on {process_id}/{node}"
                    )));
                }
            }
        }

        // Ownership, work, plan, dispatch — the standard operation, so the action
        // path carries no claim, commit, wake, projection or slot code of its
        // own (see [`operation`]). The slot policy does not apply here: the decision
        // is already durable, the intent was committed above.
        let Some(mut claimed) = self.prepare(&operation_id, operation::Slot::Direct).await? else {
            return Err(conflict(format!(
                "action intent {operation_id:?} is not claimable"
            )));
        };
        let driver = operation::driver(OperationKind::ApplyAction)
            .expect("ApplyAction is a driven operation kind");
        let mut claimed_operation = claimed.operation(&mut lane);
        let (plan, effect_started) = match driver.run(self, &mut claimed_operation).await {
            Ok(outcome) => {
                let effect_started = outcome.ran_external_effect();
                (
                    driver.next(self, &claimed_operation, outcome).await,
                    effect_started,
                )
            }
            Err(error) => (Err(error), false),
        };
        match plan {
            // The action's own commit, never a fresh load: the read model and
            // the slot follow the snapshot that commit returned, exactly like
            // every other operation's. The run's rows whole — an action is decided
            // against the aggregate as a whole, not as one operation's effect.
            Ok(plan) => {
                self.dispatch(
                    &mut claimed,
                    plan,
                    operation::Projection::Whole,
                    effect_started,
                )
                .await
            }
            Err(error) => {
                // An effect that may already have run cannot be retried
                // automatically. Effect-free plans (the state moved between
                // intent and ownership) are safe to hand back for recovery.
                if effect_started {
                    if let Err(mark_error) = self.mark_effect_unknown(&claimed).await {
                        tracing::warn!(
                            id = ?claimed.id,
                            error = %mark_error,
                            "marking an effect-unknown operation failed; its lease expiry will decide it"
                        );
                    }
                } else {
                    let _ = self.release_claim(&claimed, 1).await;
                }
                Err(ActError::Runtime(format!("action re-plan failed: {error}")))
            }
        }
    }

    /// The bare operation plan: the operation's own commit plus its outbox note.
    /// State changes and successor operations are layered on by the callers.
    #[allow(clippy::too_many_arguments)]
    fn task_operation_plan(
        &self,
        snapshot: &ProcessSnapshot,
        id: &OperationId,
        task_id: &TaskId,
        task: Option<&TaskSnapshot>,
        claim: &OperationCommit,
        note: &str,
        topic: &str,
    ) -> Result<TransitionPlan> {
        task.ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
        Ok(TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: vec![
                crate::scheduler::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "{note}:{}:{}:{}",
                        id.process_id,
                        id.task.node.as_str(),
                        id.task.generation
                    ),
                    topic: topic.into(),
                    payload: String::new(),
                },
            ],
        })
    }

    /// The `(uses, raw params, node name, model id)` of a `uses` step node,
    /// from the durable model — the act message's and the effect seam's
    /// package facts.
    fn step_uses(
        &self,
        snapshot: &ProcessSnapshot,
        task_id: &TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<Option<(String, serde_json::Value, String, String)>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            // A pushed act is not in the model: its facts ride the
            // execution's scope (see `push_plan`).
            let Some(act) = crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)?
            else {
                return Ok(None);
            };
            return Ok(Some((act.uses, act.params, act.name, model.id.clone())));
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
    /// `Task::create_message` — the node level (`act`), the resolved params and
    /// options, the step and model inputs, the scope inputs the message
    /// carries, then the operation's own outputs or its error. `facts` are the
    /// start operation's pinned resolution
    /// (absent for a host without a runner or an aggregate seeded before it);
    /// `outputs` are the operation's own scope writes (`__`-private facts and the
    /// error variables excluded); `error` is the failure the operation decided.
    #[allow(clippy::too_many_arguments)]
    fn act_message_payload(
        &self,
        snapshot: &ProcessSnapshot,
        task_id: &TaskId,
        node_id: &str,
        facts: Option<&ActFacts>,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        let node = NodeId::parse(node_id).map_err(runtime)?;
        let Some((uses, raw_params, name, mid)) = self.step_uses(snapshot, task_id, &node)? else {
            return Ok(None);
        };
        // The resolved facts when the start operation pinned them, the raw
        // declaration otherwise (`params` before a runner ever saw the step).
        let params = facts
            .map(|facts| facts.params.clone())
            .unwrap_or(raw_params);
        let options = facts
            .map(|facts| facts.options.clone())
            .unwrap_or(serde_json::Value::Null);

        let mut inputs = input_vars(snapshot, task_id);
        // The act folds into its step node in the kernel, so the step the
        // legacy message pointed at is the node itself.
        inputs.insert(
            crate::utils::consts::STEP_KEY.to_string(),
            serde_json::json!({
                crate::utils::consts::STEP_NODE_ID: node_id,
                crate::utils::consts::STEP_NODE_NAME: name,
                // The instance's external id — the wire since C5 (the legacy
                // `$step.task_id` was the legacy task's own id, not the node).
                crate::utils::consts::STEP_TASK_ID: task_id.wire(&snapshot.process_id),
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
            "tid": task_id.wire(&snapshot.process_id),
            "nid": node_id,
            "name": name,
            // The level is explicit since C6: the act payload used to leave
            // `kind` to the consumer's "act" default. The word is unchanged.
            "kind": NodeKind::Act.to_string(),
            "uses": uses,
            "mid": mid,
            "inputs": serde_json::Value::Object(inputs),
        });
        if let Some((code, message)) = error {
            payload["error"] = serde_json::json!({ "code": code, "message": message });
        } else if let Some(outputs) = outputs {
            payload["outputs"] = message_outputs(outputs, task_id);
        }
        Ok(Some(payload))
    }

    /// The task-level message payload of one execution *without* a package: the
    /// fact set the driver's transport maps onto the engine's `Message`. Every
    /// execution announces its terminal states — skipped, completed, aborted,
    /// failed and the rest — whatever its level, so a client can rebuild the
    /// run's flow from the message stream alone: a package-less step and the
    /// workflow root announce like the legacy `on_task` did, and a branch
    /// announces too (the legacy tree kept branches silent; the kernel's
    /// message stream is the trace, so a skipped or completed branch is a
    /// state the client must see — its `kind` tells the two levels apart).
    fn task_message_payload(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        node: &str,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let facts = node_facts(&model, node);
        let mut inputs = input_vars(snapshot, task_id);
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
        // The declared options (the legacy `Task::create_message` options
        // set): a package-less step announced the options its declaration
        // carried, which is what a channel filter's `tag` matches. A `uses`
        // step's act payload carries the resolved options already.
        let options = find_step_in_model(&model, node)
            .map(|step| step.options.clone())
            .unwrap_or_default();
        if !options.is_empty() {
            inputs.insert(
                crate::utils::consts::ACT_OPTIONS_KEY.to_string(),
                serde_json::to_value(options).unwrap_or(serde_json::Value::Null),
            );
        }
        let mut payload = serde_json::json!({
            "tid": task_id.wire(&snapshot.process_id),
            "nid": node,
            "name": facts.name,
            "kind": facts.message_kind.to_string(),
            "inputs": serde_json::Value::Object(inputs),
        });
        if let Some((code, message)) = error {
            payload["error"] = serde_json::json!({ "code": code, "message": message });
        } else if let Some(outputs) = outputs {
            payload["outputs"] = message_outputs(outputs, task_id);
        }
        Ok(Some(payload))
    }

    /// The message payload of one operation's record: the act payload for a `uses`
    /// step, the task payload otherwise. Both are the same legacy emission
    /// (`Task::create_message`); only the fact set differs.
    fn operation_message_payload(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        node: &str,
        facts: Option<&ActFacts>,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        if let Some(payload) =
            self.act_message_payload(snapshot, task_id, node, facts, outputs, error)?
        {
            return Ok(Some(payload));
        }
        self.task_message_payload(snapshot, task_id, node, outputs, error)
    }

    /// The variable space the evaluation seams see for one execution: its
    /// scope chain (nearest wins), the run's durable env and its caller
    /// authority, on the acting node of the declaring model.
    fn scope_spec(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model: crate::model::Workflow,
    ) -> Result<scope::ScopeSpec> {
        // The pushed act's synthetic step, when the acting node is one: the
        // declaring model has no node for it.
        let pushed = if find_step_in_model(&model, task_id.node.as_str()).is_none() {
            crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)?
                .map(|act| act.step(task_id.node.as_str()))
        } else {
            None
        };
        // An effect scope resolves parent → root names from the shared
        // aggregate on demand; contexts without a shared handle keep the
        // eager seed (predicate fallback, read-model contexts, tests).
        let (vars, shared_snapshot) = match shared {
            Some(shared) => (Vec::new(), Some(Arc::clone(shared))),
            None => (scope_vars(snapshot, task_id), None),
        };
        Ok(scope::ScopeSpec {
            process_id: snapshot.process_id.clone(),
            node: task_id.node.as_str().to_owned(),
            task_id: task_id.clone(),
            model,
            pushed,
            vars,
            snapshot: shared_snapshot,
            env: projection::env_map(snapshot)?,
            owner: owner_of(snapshot)?,
        })
    }

    /// The effect request for a `uses` step, when a runner is attached:
    /// `None` for a leaf without a package or a host running without the seam.
    fn step_effect(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<Option<(Arc<dyn effects::EffectRunner>, effects::EffectRequest)>> {
        let Some(runner) = self.effect.read().clone() else {
            return Ok(None);
        };
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            // A pushed act is not in the model: its facts ride the
            // execution's scope (see `push_plan`).
            let Some(act) = crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)?
            else {
                return Ok(None);
            };
            return Ok(Some((
                runner,
                effects::EffectRequest {
                    scope: self.scope_spec(snapshot, shared, task_id, model)?,
                    package: act.uses,
                },
            )));
        };
        let Some(package) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some((
            runner,
            effects::EffectRequest {
                scope: self.scope_spec(snapshot, shared, task_id, model)?,
                package,
            },
        )))
    }

    /// Whether a waiting act (`auto == "0"`) still has a package effect to
    /// run. Only a `Func` package does — the subflow, whose act starts the
    /// child and then waits for the child's return — while an `Irq` waits for
    /// its client without executing anything.
    fn step_runs_while_waiting(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<bool> {
        let Some((runner, request)) = self.step_effect(snapshot, shared, task_id, model_node)?
        else {
            return Ok(false);
        };
        Ok(runner.executes_while_waiting(&request.package))
    }

    /// Resolve one `uses` step's package at start (the legacy act's `run()`
    /// gate): the package's `run_as` and its validated params. A resolution
    /// error is the legacy act's start error.
    async fn resolve_step(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<Option<ActFacts>> {
        let Some((runner, request)) = self.step_effect(snapshot, shared, task_id, model_node)?
        else {
            return Ok(None);
        };
        let resolved = runner.resolve(&request).await?;
        Ok(Some(ActFacts {
            params: resolved.params,
            options: resolved.options,
        }))
    }

    /// Claim → prepare one operation through atomic operation events.
    async fn operation_event(
        &self,
        id: &OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<(
        crate::scheduler::core::model::operation::OperationRecord,
        crate::scheduler::durable::document_store::LoadedProcess,
    )> {
        let outcome = self
            .processes
            .commit_operation_event(
                id,
                expected_revision,
                event,
                SystemClock.now_ms(),
                self.timer_poll_ms,
            )
            .await
            .map_err(runtime)?;
        Self::operation_event_outcome(id, outcome)
    }

    /// [`Self::operation_event`] for a caller that already holds the
    /// aggregate it decided on (the claim path's pre-claim load): the commit
    /// uses those documents as its CAS token instead of reading the same
    /// aggregate a second time.
    async fn operation_event_from(
        &self,
        loaded: crate::scheduler::durable::document_store::LoadedProcess,
        id: &OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<(
        crate::scheduler::core::model::operation::OperationRecord,
        crate::scheduler::durable::document_store::LoadedProcess,
    )> {
        let outcome = self
            .processes
            .commit_operation_event_from(
                loaded,
                id,
                expected_revision,
                event,
                SystemClock.now_ms(),
                self.timer_poll_ms,
            )
            .await
            .map_err(runtime)?;
        Self::operation_event_outcome(id, outcome)
    }

    fn operation_event_outcome(
        id: &OperationId,
        outcome: crate::scheduler::durable::async_document_store::OperationCommitOutcome,
    ) -> Result<(
        crate::scheduler::core::model::operation::OperationRecord,
        crate::scheduler::durable::document_store::LoadedProcess,
    )> {
        match outcome {
            crate::scheduler::durable::async_document_store::OperationCommitOutcome::Committed { operation, loaded }
            | crate::scheduler::durable::async_document_store::OperationCommitOutcome::AlreadyCommitted { operation, loaded } => {
                Ok((operation, loaded))
            }
            crate::scheduler::durable::async_document_store::OperationCommitOutcome::Conflict { .. } => {
                Err(conflict(format!("claim conflict on operation {id:?}")))
            }
        }
    }

    async fn load(
        &self,
        process_id: &str,
    ) -> Result<Option<crate::scheduler::durable::document_store::LoadedProcess>> {
        self.processes.load(process_id).await.map_err(runtime)
    }

    pub(crate) async fn snapshot(
        &self,
        process_id: &str,
    ) -> Result<Option<crate::scheduler::core::commit::aggregate::ProcessSnapshot>> {
        Ok(self
            .load(process_id)
            .await?
            .map(|loaded| std::sync::Arc::unwrap_or_clone(loaded.snapshot)))
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
            .is_some_and(|snapshot| decision::run_settled(&snapshot)))
    }
}

#[cfg(test)]
mod tests;
