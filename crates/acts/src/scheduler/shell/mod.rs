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

use self::health::{LoopGuard, LoopHealth};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender, channel};

// The shell's parts are its own submodules: they see this module's scope and
// its private items, and this module re-exports the ones the runtime names
// through it.
mod action;
mod context;
mod effects;
mod errors;
mod facts;
pub(crate) mod health;
mod message;
mod model;
mod operation;
mod planning;
mod predicate;
mod process;
mod projection;
mod scope;
mod task_tree;
pub(crate) mod validation;

use self::model::task_graph;
use self::{action::*, errors::*, facts::*, planning::*};
pub use context::Context;
pub(crate) use effects::PackageEffectRunner;
pub(crate) use message::MessageForwarder;
pub(crate) use model::ProcessSpec;
pub(crate) use predicate::EnginePredicateRunner;
pub(crate) use process::{Process, Task};
pub(crate) use projection::{ProjectionMetrics, RowProjector};

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

/// The executor's proof about one `EffectUnknown` operation (A1c): the fact
/// Core's recovery decision reads, plus the payload only the shell can hand to
/// the normal effect planner when the decision says `ReplayEffect`.
struct ObservedEffect {
    receipt: EffectReceipt,
    observation: Option<effects::EffectObservation>,
}

/// The fence one delivery attempt uses. The normal path applies it only in
/// memory before its batch confirmation; the retry pass persists it first, so
/// a concurrent worker cannot deliver the same record under the same fence.
fn delivery_claim(record: &OutboxRecord, now: i64) -> DeliveryClaim {
    DeliveryClaim {
        owner: OWNER.into(),
        fence: record.last_fence + 1,
        lease_until_ms: now + DELIVERY_LEASE_MS,
        heartbeat_revision: 0,
    }
}

/// The state-machine event one transport outcome becomes. `Quarantine` is
/// valid from `Pending` directly; every other outcome needs the claim first.
fn outcome_event(claim: &DeliveryClaim, outcome: DeliveryOutcome, now: i64) -> OutboxEvent {
    match outcome {
        DeliveryOutcome::Delivered { receipt } => OutboxEvent::ConfirmDelivered {
            claim: claim.clone(),
            receipt,
            now_ms: now,
        },
        DeliveryOutcome::RetryAt {
            available_at_ms,
            error,
        } => OutboxEvent::Reschedule {
            claim: claim.clone(),
            available_at_ms,
            error,
            now_ms: now,
        },
        DeliveryOutcome::OutcomeUnknown { reason } => OutboxEvent::MarkDeliveryUnknown {
            claim: claim.clone(),
            reason,
            now_ms: now,
        },
        DeliveryOutcome::Quarantine { reason } => OutboxEvent::Quarantine { reason },
    }
}

/// The in-memory event sequence a normal-path delivery applies before the one
/// batch commit: claim, then the transport's outcome.
fn delivery_events(claim: &DeliveryClaim, outcome: DeliveryOutcome, now: i64) -> Vec<OutboxEvent> {
    let event = outcome_event(claim, outcome, now);
    match event {
        OutboxEvent::Quarantine { .. } => vec![event],
        _ => vec![
            OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: now,
            },
            event,
        ],
    }
}

/// Best-effort wake delivery into the host's queue. Losing a wake is safe:
/// the operation also owns a durable ready record, and the driver's due pass
/// picks it up. A failed send is counted and wakes the due-pass notification.
#[derive(Clone)]
struct QueueWake {
    tx: Sender<OperationId>,
}

impl QueueWake {
    fn wake(&self, operation: &OperationId) -> bool {
        self.tx.try_send(operation.clone()).is_ok()
    }
}

/// What the driver's ready-queue passes observed. A gauge, not a history: the
/// last pass wins for backlog/age; the counters accumulate for the host's
/// lifetime.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadyMetrics {
    /// Ready records the last pass observed (due or not).
    pub backlog: usize,
    /// Operations the last page found due.
    pub due: usize,
    /// Age of the oldest observed ready record, in milliseconds.
    pub oldest_ready_age_ms: Option<i64>,
    /// Passes run over the host's lifetime.
    pub passes: u64,
    /// Index entries walked over the host's lifetime.
    pub scanned: u64,
    /// Operations deferred to a later poll (parked runs, idle timers).
    pub deferred: u64,
    /// Wakes that did not fit the in-memory channel.
    pub wake_failures: u64,
    /// Missing ready records repaired by the boot pass.
    pub repaired: u64,
}

/// What the outbox retry pass observed and did (A10). `backlog` and
/// `oldest_due_age_ms` are gauges of the last pass; the counters accumulate for
/// the host's lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OutboxMetrics {
    /// Due records the last pass saw in the index.
    pub backlog: usize,
    /// Age of the oldest due record, in milliseconds.
    pub oldest_due_age_ms: Option<i64>,
    /// Index entries the last pass walked (due or not).
    pub observed: usize,
    /// Retry passes run.
    pub passes: u64,
    /// Records delivered by a retry pass.
    pub delivered: u64,
    /// Records left for a later pass because their run's rows were behind.
    pub deferred: u64,
}

/// What the recovery scanner observed and did (A10). `scanned` is the backlog
/// examined by the last pass (every exceptional row is scanned by a pass);
/// the counters accumulate for the host's lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecoveryMetrics {
    /// Exceptional operations the last pass examined.
    pub scanned: usize,
    /// Operations left after the last pass's budget (the scan resumes there).
    pub more_pending: bool,
    pub passes: u64,
    /// Expired claims reclaimed into the ready queue.
    pub reclaimed: u64,
    /// Missing ready records repaired.
    pub ready_repaired: u64,
    /// Commit-prepared operations finalized.
    pub finalized: u64,
    /// Operations needing manual reconciliation, as of the last pass.
    pub reconciliation_required: usize,
    pub effect_applied: u64,
    pub effect_retried: u64,
}

/// The independent cadences and per-round budgets of the kernel host (A10).
///
/// Each loop owns one interval: the ready/timer pass (normal liveness), the
/// recovery scanner (exceptional work), the outbox retry pass and the
/// projection repair pass. A slow or failing loop cannot delay another, and a
/// shorter loop never multiplies another's scans.
#[derive(Clone, Copy, Debug)]
pub(crate) struct KernelSchedule {
    pub(crate) ready: Duration,
    pub(crate) recovery: Duration,
    pub(crate) outbox: Duration,
    pub(crate) projection: Duration,
    pub(crate) ready_budget: usize,
    pub(crate) recovery_budget: usize,
    pub(crate) outbox_budget: usize,
    pub(crate) projection_budget: usize,
}

impl KernelSchedule {
    /// The pre-A10 cadence: every loop on the driver tick, with the historical
    /// page budgets. Used by hosts that do not configure the split.
    #[cfg(test)]
    pub(crate) fn uniform(tick: Duration) -> Self {
        Self {
            ready: tick,
            recovery: tick,
            outbox: tick,
            projection: tick,
            ready_budget: PAGE_SIZE,
            recovery_budget: PAGE_SIZE,
            outbox_budget: PAGE_SIZE,
            projection_budget: 32,
        }
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

/// The configured bounds of the five admission categories (A8). Execution
/// bounds normal runs in flight; the other four bound their own work and never
/// consume another category's budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AdmissionCaps {
    pub(crate) execution: usize,
    pub(crate) effect: usize,
    pub(crate) recovery: usize,
    pub(crate) reconciliation: usize,
    pub(crate) projection: usize,
}

impl AdmissionCaps {
    /// The caps a host without an explicit admission configuration uses: the
    /// execution bound it was given and the process defaults for the rest.
    #[cfg(test)]
    fn new(execution: usize) -> Self {
        Self {
            execution,
            effect: crate::config::DEFAULT_EFFECT_CONCURRENCY,
            recovery: crate::config::DEFAULT_CONTROL_CONCURRENCY,
            reconciliation: crate::config::DEFAULT_CONTROL_CONCURRENCY,
            projection: crate::config::DEFAULT_CONTROL_CONCURRENCY,
        }
    }

    fn from_config(config: &crate::Config) -> Self {
        Self {
            execution: config.cache_cap() as usize,
            effect: config.effect_concurrency(),
            recovery: config.recovery_concurrency(),
            reconciliation: config.reconciliation_concurrency(),
            projection: config.projection_concurrency(),
        }
    }
}

/// One auxiliary quota's counter and waiters. The rule is
/// [`decision::quota`]; this type is only the in-memory half the shell owns.
#[derive(Debug, Default)]
struct AdmissionGate {
    cap: usize,
    active: std::sync::atomic::AtomicUsize,
    admitted: std::sync::atomic::AtomicU64,
    waiters: tokio::sync::Notify,
}

impl AdmissionGate {
    fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            cap,
            active: std::sync::atomic::AtomicUsize::new(0),
            admitted: std::sync::atomic::AtomicU64::new(0),
            waiters: tokio::sync::Notify::new(),
        })
    }

    /// Take one permit, waiting while the category is saturated. The waiter
    /// is registered before the counter is read, so a permit released in
    /// between is never lost.
    async fn enter(self: &Arc<Self>) -> AdmissionLease {
        use std::sync::atomic::Ordering::SeqCst;
        loop {
            let notified = self.waiters.notified();
            let active = self.active.load(SeqCst);
            if decision::quota(self.cap, active) == QuotaDirective::Wait {
                notified.await;
                continue;
            }
            if self
                .active
                .compare_exchange(active, active.saturating_add(1), SeqCst, SeqCst)
                .is_ok()
            {
                self.admitted.fetch_add(1, SeqCst);
                return AdmissionLease { gate: self.clone() };
            }
            // Another entrant took the permit between the read and the CAS.
            // Re-check immediately: with a cap above one a free permit may
            // still exist, and waiting here could strand it.
        }
    }

    fn metrics(&self) -> QuotaMetrics {
        QuotaMetrics {
            cap: self.cap,
            active: self.active.load(std::sync::atomic::Ordering::SeqCst),
            admitted: self.admitted.load(std::sync::atomic::Ordering::SeqCst),
        }
    }
}

/// One held auxiliary permit; dropping it wakes one waiter.
struct AdmissionLease {
    gate: Arc<AdmissionGate>,
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        self.gate
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.gate.waiters.notify_one();
    }
}

/// One auxiliary quota's gauge: the configured cap, holders now, and how many
/// holders it admitted over the host's lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaMetrics {
    pub(crate) cap: usize,
    pub(crate) active: usize,
    pub(crate) admitted: u64,
}

/// Every category's gauge (A8). Execution reports its slot set and parked
/// queue; each auxiliary quota reports its own counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AdmissionMetrics {
    pub(crate) execution: QuotaMetrics,
    pub(crate) parked: usize,
    pub(crate) effect: QuotaMetrics,
    pub(crate) recovery: QuotaMetrics,
    pub(crate) reconciliation: QuotaMetrics,
    pub(crate) projection: QuotaMetrics,
    /// Runs the last decisions observed waiting on manual reconciliation.
    pub(crate) quarantined_runs: usize,
}

/// The driver's slot bookkeeping: which runs hold one of the execution slots,
/// which wait for one, and the four auxiliary quotas (A8).
///
/// The configured number used to bound the legacy engine's *resident object
/// set*; the kernel holds no resident objects, so the same number is redefined
/// as the bound on runs that are in flight at once (see
/// [`concurrency`](crate::scheduler::core::plan::concurrency) for the rule).
/// A run that cannot take a slot *parks*: a start seeds it with its root still
/// `Created` — the read model's `None` process state, exactly the legacy parked
/// row — and leaves that root's operation undriven, while a run that was
/// already in flight keeps the row it had (the legacy boot-resume overflow's
/// shape). Either way the run takes no operation, and the oldest parked run
/// (`__created` ascending, the legacy row's `timestamp` order) is what a freed
/// slot admits.
///
/// The execution slot is *not* a lease on a run forever: a run that settles,
/// or one whose only remaining route is manual reconciliation, releases it in
/// the decision that observed that fact. Effects, recovery, human
/// reconciliation and projections have their own gates, so a saturated
/// category never holds another category's work.
///
/// The legacy resident set answered the same question through `Cache`; here
/// the only writers are [`KernelHost::start_process`] (which reserves) and the
/// driver's operations (which settle), so this set is the single answer to "may
/// this run operation". Like that set it is in-memory: a restart starts it
/// empty, and the runs still in flight take their slots again through
/// [`KernelHost::place_run`] when the ready queue hands them their first turn.
/// The first `cap` of them are admitted in the order the ready index returns
/// them rather than by `__created` (the store keeps no boot index to sort by),
/// while every later admission is by stamp, exactly like the legacy parked
/// queue.
#[derive(Default)]
struct Admissions {
    /// The bound on runs in flight at once.
    cap: usize,
    /// Runs holding an execution slot: admitted here and not yet settled or
    /// quarantined.
    active: std::collections::HashSet<String>,
    /// Execution slots granted over the host's lifetime (a metric).
    admitted: u64,
    /// Parked runs, oldest first, with the operation that admits each: the
    /// seeded root `ExecuteTask` whose operation starts the parked root. The key is
    /// the run's `__created` stamp; two runs seeded in the same millisecond
    /// are ordered by pid, so no run can hide behind an equal key.
    parked: std::collections::BTreeMap<(i64, String), OperationId>,
    /// Concurrent external effects.
    effect: Arc<AdmissionGate>,
    /// Recovery scans and their exceptional commands.
    recovery: Arc<AdmissionGate>,
    /// Human reconciliation of `EffectUnknown`/`Quarantined` operations.
    reconciliation: Arc<AdmissionGate>,
    /// Row projections and their repair work.
    projection: Arc<AdmissionGate>,
    /// Runs whose remaining route is manual reconciliation, as the last
    /// decision observed them. A gauge for operators; the admission rule reads
    /// the aggregate, never this set.
    quarantined: std::collections::HashSet<String>,
}

impl Admissions {
    fn new(caps: AdmissionCaps) -> Self {
        Self {
            cap: caps.execution,
            effect: AdmissionGate::new(caps.effect),
            recovery: AdmissionGate::new(caps.recovery),
            reconciliation: AdmissionGate::new(caps.reconciliation),
            projection: AdmissionGate::new(caps.projection),
            ..Self::default()
        }
    }

    /// The gate for an auxiliary category. Execution has no auxiliary gate:
    /// its set is the ledger itself and its rule is [`decision::place`].
    fn gate(&self, category: AdmissionCategory) -> Option<Arc<AdmissionGate>> {
        match category {
            AdmissionCategory::Execution => None,
            AdmissionCategory::Effect => Some(self.effect.clone()),
            AdmissionCategory::Recovery => Some(self.recovery.clone()),
            AdmissionCategory::Reconciliation => Some(self.reconciliation.clone()),
            AdmissionCategory::Projection => Some(self.projection.clone()),
        }
    }

    fn metrics(&self) -> AdmissionMetrics {
        AdmissionMetrics {
            execution: QuotaMetrics {
                cap: self.cap,
                active: self.active.len(),
                admitted: self.admitted,
            },
            parked: self.parked.len(),
            effect: self.effect.metrics(),
            recovery: self.recovery.metrics(),
            reconciliation: self.reconciliation.metrics(),
            projection: self.projection.metrics(),
            quarantined_runs: self.quarantined.len(),
        }
    }

    /// Record the last decision's view of a run's reconciliation hold. The
    /// admission rule itself reads the aggregate; this set is the operator
    /// gauge.
    fn note_quarantined(&mut self, process_id: &str, quarantined: bool) {
        if quarantined {
            self.quarantined.insert(process_id.to_string());
        } else {
            self.quarantined.remove(process_id);
        }
    }

    /// The pure rule against the set it bounds.
    fn decide(&self, state: crate::scheduler::core::model::task_graph::TaskState) -> Admission {
        infallible(Admit::plan((), &(self.cap, self.active.len(), state)))
    }

    /// Reserve a slot for a fresh run — its root is seeded `Ready`, which is
    /// the state the rule is asked about — or decide that it parks. The
    /// reservation and the decision are one step, so concurrent starts cannot
    /// both take the last slot.
    fn reserve(&mut self, process_id: &str) -> Admission {
        let admission = self.decide(crate::scheduler::core::model::task_graph::TaskState::Ready);
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
        if self.active.insert(process_id.to_string()) {
            self.admitted = self.admitted.saturating_add(1);
        }
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
        self.quarantined.remove(process_id);
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

    /// A run that no longer needs an execution slot frees it; returns the
    /// operation that admits the parked run taking it, if there is one. The
    /// caller wakes that operation immediately — the tick is only a fallback.
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

    /// The core admission policy over this in-memory set (A6/A8): the shell
    /// owns the set, [`crate::scheduler::core::decision::place`] owns the rule.
    fn directive(&self, snapshot: &ProcessSnapshot) -> AdmissionDirective {
        let standing = if self.is_parked(&snapshot.process_id) {
            SlotStanding::Parked
        } else if self.holds(&snapshot.process_id) {
            SlotStanding::Holding
        } else {
            SlotStanding::Unplaced
        };
        decision::place(
            SlotObservation {
                standing,
                cap: self.cap,
                active: self.active.len(),
            },
            RunFacts::of(snapshot),
        )
    }

    /// Execute one admission directive against the set. Returns the operation
    /// that admits the parked run taking a freed slot, if any.
    fn apply(
        &mut self,
        snapshot: &ProcessSnapshot,
        directive: AdmissionDirective,
    ) -> Option<OperationId> {
        match directive {
            AdmissionDirective::Park => {
                if !self.is_parked(&snapshot.process_id) {
                    self.park(
                        created_stamp(snapshot),
                        &snapshot.process_id,
                        &decision::resume_operation(snapshot),
                    );
                }
                None
            }
            AdmissionDirective::Drive(SlotDirective::Release) => self.release(&snapshot.process_id),
            AdmissionDirective::Drive(SlotDirective::Take) => {
                self.activate(&snapshot.process_id);
                None
            }
            AdmissionDirective::Drive(SlotDirective::Keep | SlotDirective::NoSlot) => None,
        }
    }
}

/// The default outbox transport: it logs each record and confirms it — what a
/// host with nothing attached (the kernel's own tests, and any embedder that
/// only wants the durable aggregate) does. The serving host replaces it with
/// [`message::MessageForwarder`] so records reach the engine channel.
struct ShadowTransport;

impl crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport for ShadowTransport {
    fn deliver(
        &mut self,
        record: &crate::scheduler::core::outbox::records::OutboxRecord,
    ) -> crate::scheduler::core::outbox::outbox_dispatch::DeliveryOutcome {
        tracing::debug!(
            pid = %record.id.process_id,
            key = %record.id.key,
            topic = %record.topic,
            "kernel outbox delivered (shadow)"
        );
        crate::scheduler::core::outbox::outbox_dispatch::DeliveryOutcome::Delivered {
            receipt: "shadow".into(),
        }
    }
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

    /// The caps a [`Config`](crate::Config) names (A8). Public entry points
    /// that build a host without one use [`AdmissionCaps::new`]'s defaults.
    pub(crate) fn caps_from_config(config: &crate::Config) -> AdmissionCaps {
        AdmissionCaps::from_config(config)
    }

    /// Take one permit of an auxiliary admission category. Execution has no
    /// auxiliary gate — its set is the ledger and [`Self::place_run`] is its
    /// admission path.
    async fn enter_admission(&self, category: AdmissionCategory) -> AdmissionLease {
        assert!(
            category != AdmissionCategory::Execution,
            "execution admission is the slot ledger, not a gate"
        );
        let gate = self
            .admissions
            .lock()
            .gate(category)
            .expect("execution admission is the slot ledger, not a gate");
        gate.enter().await
    }

    /// Every admission category's gauge (A8).
    pub(crate) fn admission_metrics(&self) -> AdmissionMetrics {
        self.admissions.lock().metrics()
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
                scope: self.scope_spec(snapshot, task_id, model)?,
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
        touched: &std::collections::BTreeSet<String>,
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
                    self.report_lifecycle(&loaded.snapshot);
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
                Ok(Some(loaded)) => match projector.project_repair(&loaded.snapshot).await {
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
                },
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

    /// Serialize a start and the cleanup of the same pid (see
    /// [`Self::lifecycle_locks`]). A start holds it from the
    /// aggregate-existence check through creation; cleanup holds it from the
    /// authoritative batch through the row/workdir removal.
    pub(crate) async fn lock_lifecycle(
        &self,
        process_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.lifecycle_locks.lock();
            locks
                .entry(process_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
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

    /// Mark that a pid's aggregate was deleted and its residue is still being
    /// removed; a start refuses the pid until [`Self::finish_reclaim`].
    pub(crate) fn begin_reclaim(&self, process_id: &str) {
        self.reclaiming.lock().insert(process_id.to_string());
    }

    /// The residue is gone (or the run turned out not to be reclaimable).
    pub(crate) fn finish_reclaim(&self, process_id: &str) {
        self.reclaiming.lock().remove(process_id);
    }

    /// Whether a cleanup of this pid started and has not finished. A start
    /// checks this under the lifecycle lock.
    pub(crate) fn is_reclaiming(&self, process_id: &str) -> bool {
        self.reclaiming.lock().contains(process_id)
    }

    /// The pids whose residue removal is still pending, for the cleanup pass
    /// to clear after a failure whose rows turned out to be gone.
    pub(crate) fn reclaiming_pids(&self) -> Vec<String> {
        self.reclaiming.lock().iter().cloned().collect()
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

    /// Report one run's lifecycle from a fresh snapshot, at most once per
    /// phase. Mirrors the legacy `on_proc`: a start while the run is alive
    /// (running or pending); otherwise — every execution terminal — an error
    /// for a failed root, a completion for any other terminal, with the
    /// model's `exposes` validated against the run's outputs (a validation
    /// failure is reported as an error, exactly like the legacy handler).
    ///
    /// The root is announced twice on purpose: this sink is the legacy
    /// `on_proc`, while the root's own outbox records (`message.created` with
    /// `kind=workflow`, then its terminal topics) are the legacy `on_task`.
    /// The legacy engine emitted both, so both paths stay — neither replaces
    /// the other, and they carry different fact sets (`process_message` here,
    /// the task payload there).
    fn report_lifecycle(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    ) {
        let directive = decision::lifecycle(snapshot);
        self.report_lifecycle_decision(snapshot, directive.as_ref());
    }

    /// Execute the lifecycle directive Core already decided. The shell owns
    /// only the message shape, the sink and the once-per-phase bookkeeping;
    /// *whether* the run is alive or settled (and whether a terminal root is an
    /// error) is [`crate::scheduler::core::decision::lifecycle`].
    fn report_lifecycle_decision(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        directive: Option<&LifecycleDirective>,
    ) {
        let lifecycle = self.lifecycle.read();
        let Some(sink) = lifecycle.as_ref() else {
            return;
        };
        let Some(root) = root_task(snapshot) else {
            return;
        };
        let Some(directive) = directive else {
            return;
        };
        let mut reported = self.reported.lock();
        match directive {
            LifecycleDirective::Started => {
                if reported.started.insert(snapshot.process_id.clone()) {
                    let message = process_message(snapshot, &root);
                    sink(&message, ProcessEvent::Started);
                }
            }
            LifecycleDirective::Completed => {
                if !reported.finished.insert(snapshot.process_id.clone()) {
                    return;
                }
                let mut message = process_message(snapshot, &root);
                // The legacy `on_proc` handler validates the model's
                // `exposes` against the completed outputs; a mismatch is
                // reported as an error on the same completion event.
                match exposes_error(snapshot, &message) {
                    Some(error) => {
                        message.set_err("", &error);
                        sink(&message, ProcessEvent::Errored);
                    }
                    None => sink(&message, ProcessEvent::Completed),
                }
            }
            LifecycleDirective::Errored { code, message } => {
                if !reported.finished.insert(snapshot.process_id.clone()) {
                    return;
                }
                let mut payload = process_message(snapshot, &root);
                payload.set_err(code, message);
                sink(&payload, ProcessEvent::Errored);
            }
        }
    }

    /// The background driver (A10): executes woken operations as they arrive,
    /// and runs each control-plane loop on its own interval —
    /// [`KernelSchedule::ready`] for the durable ready queue and timers,
    /// [`KernelSchedule::recovery`] for the exceptional scan,
    /// [`KernelSchedule::outbox`] for delivery retries, and
    /// [`KernelSchedule::projection`] for row repair. Notifications still
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
        for ticker in [
            &mut ready_ticker,
            &mut recovery_ticker,
            &mut outbox_ticker,
            &mut projection_ticker,
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

    /// Wake one operation. A wake is an optimization over the durable ready
    /// index: a full queue drops it, the ready record remains, and the due pass
    /// is notified to pick it up before the next tick.
    fn wake(&self, operation: &OperationId) -> bool {
        let sent = QueueWake {
            tx: self.wake_tx.clone(),
        }
        .wake(operation);
        if !sent {
            self.ready_metrics.lock().wake_failures += 1;
            self.ready.notify_one();
        }
        sent
    }

    pub(crate) fn ready_metrics(&self) -> ReadyMetrics {
        *self.ready_metrics.lock()
    }

    /// The slot bookkeeping for one run, from a fresh snapshot of it. Returns
    /// whether the operation that asked may run.
    ///
    /// - a run this driver admitted and whose executions all settled frees its
    ///   slot, and the parked run that takes it is returned for admission;
    /// - a parked run keeps waiting: its operations stay pending and undriven
    ///   until a slot frees;
    /// - a run this driver never admitted — one already in flight when it
    ///   booted, since every start reserves through [`Self::start_process`] —
    ///   takes a slot here, or parks exactly like a start that found every
    ///   slot taken. This is the boot pass: the ready queue hands every
    ///   unfinished run its first turn, and this is where the cap meets it.
    fn place_run(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    ) -> (bool, Option<OperationId>) {
        let mut admissions = self.admissions.lock();
        // The rule is core's (`decision::place`); this set is only the state
        // it reads and the mutations it names.
        let directive = admissions.directive(snapshot);
        let freed = admissions.apply(snapshot, directive);
        (directive.drives(), freed)
    }

    /// Make a parked run's ready record due now, then wake it. Used when a
    /// freed slot admits it: its record was deferred while it waited, and the
    /// wake is only the accelerator over that durable record.
    async fn admit_run(&self, operation: &OperationId) -> Result<()> {
        self.processes
            .set_ready_at(operation, SystemClock.now_ms())
            .await
            .map_err(runtime)?;
        self.wake(operation);
        Ok(())
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
                let package_step =
                    !modelled || self.step_uses(snapshot, &task.model_node)?.is_some();
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
    /// tell the instances apart — see the registered boundary on
    /// node-addressed actions in `KERNEL_CUTOVER_NOTES.md`.
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
        let Some(claimed) = self.prepare(&operation_id, operation::Slot::Direct).await? else {
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
                self.dispatch(&claimed, plan, operation::Projection::Whole, effect_started)
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
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
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
        let Some((uses, raw_params, name, mid)) = self.step_uses(snapshot, &node)? else {
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
    /// fact set the driver's transport maps onto the engine's `Message`. The
    /// legacy engine announced every task it created, settled or skipped — not
    /// only the acts (its `on_task` emits each state change that is neither
    /// `pending` nor `running`) — with `type` = the node kind and `name` the
    /// node's name.
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
        // A branch task emits nothing: the legacy `Branch::init` clears its
        // emit flag and never re-enables it (only an act's `run` does), so a
        // branch's own state changes are invisible to clients on both engines.
        if facts.message_kind == NodeKind::Branch {
            return Ok(None);
        }
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
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model: crate::model::Workflow,
    ) -> Result<scope::ScopeSpec> {
        Ok(scope::ScopeSpec {
            process_id: snapshot.process_id.clone(),
            node: task_id.node.as_str().to_owned(),
            task_id: task_id.clone(),
            model,
            vars: scope_vars(snapshot, task_id),
            env: projection::env_map(snapshot)?,
            owner: owner_of(snapshot)?,
        })
    }

    /// The effect request for a `uses` step, when a runner is attached:
    /// `None` for a leaf without a package or a host running without the seam.
    fn step_effect(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
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
            return Ok(None);
        };
        let Some(package) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some((
            runner,
            effects::EffectRequest {
                scope: self.scope_spec(snapshot, task_id, model)?,
                package,
            },
        )))
    }

    /// Resolve one `uses` step's package at start (the legacy act's `run()`
    /// gate): the package's `run_as` and its validated params. A resolution
    /// error is the legacy act's start error.
    async fn resolve_step(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<Option<ActFacts>> {
        let Some((runner, request)) = self.step_effect(snapshot, task_id, model_node)? else {
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
        match self
            .processes
            .commit_operation_event(
                id,
                expected_revision,
                event,
                SystemClock.now_ms(),
                self.timer_poll_ms,
            )
            .await
            .map_err(runtime)?
        {
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

    /// Reclaim one finished run's durable state: the aggregate and its
    /// recovery/outbox/ready index documents are deleted in one guarded batch
    /// (see [`AsyncProcessRepository::reclaim`]). The caller is responsible for
    /// the preconditions that live outside the aggregate (channel delivery
    /// rows); this method decides only what the aggregate itself says.
    ///
    /// On a successful reclaim (or an aggregate that is already gone) the
    /// host forgets its in-process bookkeeping for the pid — reported
    /// lifecycle phases, admission slots and projection version claims — so a
    /// later start under the same pid is a fresh run. `Retained` leaves every
    /// fact exactly as it was.
    pub(crate) async fn reclaim_process(&self, process_id: &str) -> Result<ReclaimOutcome> {
        let outcome = self
            .processes
            .reclaim(process_id)
            .await
            .map_err(repository_error)?;
        if !matches!(outcome, ReclaimOutcome::Retained) {
            self.forget_process(process_id);
        }
        Ok(outcome)
    }

    /// Drop the pid-keyed in-process state of a reclaimed run. Called only
    /// once the durable aggregate is gone (or was already gone): the pid is a
    /// new identity from here on, and the next start must not inherit a
    /// reported lifecycle phase, a slot or a projection claim.
    fn forget_process(&self, process_id: &str) {
        self.admissions.lock().abandon(process_id);
        {
            let mut reported = self.reported.lock();
            reported.started.remove(process_id);
            reported.finished.remove(process_id);
        }
        // A handle still waiting on the reclaimed run must not wait forever:
        // its re-read finds no aggregate and reports the PID as gone.
        self.notify_settled(process_id);
        let projector = self.projection.read().clone();
        if let Some(projector) = projector {
            projector.forget_process(process_id);
        }
    }

    /// Clear a reclaimed run's projection tombstone before a new run takes the
    /// pid (see [`projection::RowProjector::forget_process`]).
    fn resume_process(&self, process_id: &str) {
        let projector = self.projection.read().clone();
        if let Some(projector) = projector {
            projector.resume(process_id);
        }
    }

    /// Whether every execution of the process reached a terminal state.
    #[cfg(test)]
    pub(crate) async fn process_completed(&self, process_id: &str) -> Result<bool> {
        Ok(self
            .snapshot(process_id)
            .await?
            .is_some_and(|snapshot| decision::run_settled(&snapshot)))
    }

    /// The wakeup a durable run handle registers before re-reading the
    /// aggregate (see [`crate::RunHandle::wait`]).
    ///
    /// The entry is per pid and taken by [`Self::notify_settled`], so a waiter
    /// that arrives after the settle gets a fresh entry and sees the terminal
    /// aggregate on its own re-read: the notification only ever shortens the
    /// wait, it never carries the outcome.
    pub(crate) fn run_waiter(&self, process_id: &str) -> Arc<tokio::sync::Notify> {
        self.settled
            .lock()
            .entry(process_id.to_string())
            .or_default()
            .clone()
    }

    /// Wake every handle waiting on `process_id`: the run's commit settled it
    /// (its aggregate is durable and readable) or its durable record was
    /// reclaimed. Taking the entry keeps the map bounded by pids actually
    /// being waited on.
    fn notify_settled(&self, process_id: &str) {
        let waiter = self.settled.lock().remove(process_id);
        if let Some(waiter) = waiter {
            waiter.notify_waiters();
        }
    }

    /// Ask the executor for its proof about one `EffectUnknown` operation
    /// (A1c).
    ///
    /// This is the observation half of recovery: it performs the executor IO
    /// by the operation's stable effect key and converts the answer into the
    /// fact Core's `RecoverOperation` reads. It never decides what the answer
    /// means — replay, retry and manual are [`recover_operation`]'s call.
    async fn observe_effect(&self, observed: &Observed) -> Result<Option<ObservedEffect>> {
        if observed.kind != OperationKind::ApplyAction
            || !matches!(
                observed.persisted_phase,
                OperationState::EffectUnknown { .. }
            )
        {
            return Ok(None);
        }
        let Some(loaded) = self.load(&observed.operation_id.process_id).await? else {
            return Ok(None);
        };
        let Some(task) = loaded.snapshot.tasks.get(&observed.operation_id.task) else {
            return Ok(None);
        };
        let model_node = task.model_node.clone();
        let Some((runner, request)) =
            self.step_effect(&loaded.snapshot, &observed.operation_id.task, &model_node)?
        else {
            return Ok(None);
        };
        let key = EffectKey::of(&observed.operation_id);
        let observation = match runner.observe(&key).await {
            Ok(observation) => observation,
            Err(error) => {
                tracing::warn!(
                    operation = ?observed.operation_id,
                    error = %error,
                    "querying the effect observation failed; manual reconciliation still owns the operation"
                );
                return Ok(Some(ObservedEffect {
                    receipt: EffectReceipt::Unknown,
                    observation: None,
                }));
            }
        };
        let receipt = match &observation {
            effects::EffectObservation::Applied(_) => EffectReceipt::Applied,
            effects::EffectObservation::Failed(_) => EffectReceipt::Failed,
            // Re-running can only be safe when the effect itself is provably
            // idempotent; the executor keeps the same key, so a duplicate
            // delivery is deduplicated on its side.
            effects::EffectObservation::NotApplied => EffectReceipt::NotApplied {
                idempotent: runner.idempotent(&request),
            },
            effects::EffectObservation::Unknown => EffectReceipt::Unknown,
        };
        let observation = matches!(receipt, EffectReceipt::Applied | EffectReceipt::Failed)
            .then_some(observation);
        Ok(Some(ObservedEffect {
            receipt,
            observation,
        }))
    }

    /// The unknown effects of one run, in operation order (A1c).
    pub(crate) async fn effect_unknowns(
        &self,
        process_id: &str,
    ) -> Result<Vec<crate::engine::EffectUnknown>> {
        let Some(snapshot) = self.snapshot(process_id).await? else {
            return Ok(Vec::new());
        };
        let mut unknowns = snapshot
            .operations
            .values()
            .filter_map(|record| match record.state {
                OperationState::EffectUnknown { attempt } => Some(crate::engine::EffectUnknown {
                    pid: process_id.to_string(),
                    tid: record.id.task.wire(process_id),
                    sequence: record.id.sequence,
                    key: EffectKey::of(&record.id).wire(),
                    attempt,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        unknowns.sort_by(|left, right| {
            left.sequence
                .cmp(&right.sequence)
                .then_with(|| left.tid.cmp(&right.tid))
        });
        Ok(unknowns)
    }

    /// Resolve one unknown effect by its stable key (A1c).
    pub(crate) async fn resolve_effect_by_key(
        &self,
        process_id: &str,
        key: &str,
        resolution: crate::engine::EffectResolution,
    ) -> Result<()> {
        let snapshot = self
            .snapshot(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {process_id} not found")))?;
        let operation = snapshot
            .operations
            .values()
            .find(|record| EffectKey::of(&record.id).wire() == key)
            .map(|record| record.id.clone())
            .ok_or_else(|| {
                ActError::Runtime(format!(
                    "no operation with effect key {key:?} in run {process_id}"
                ))
            })?;
        self.resolve_effect(&operation, resolution).await
    }

    /// The operator's durable decision for one `EffectUnknown` operation
    /// (A1c): replay it as applied (with the outputs the operator captured),
    /// or prove it never ran so it may be retried.
    pub(crate) async fn resolve_effect(
        &self,
        id: &OperationId,
        resolution: crate::engine::EffectResolution,
    ) -> Result<()> {
        // A8: a human decision is control-plane work with its own budget. It
        // never asks for an execution slot, so a full run quota cannot park it
        // and it cannot park a new run.
        let _reconciliation = self
            .enter_admission(AdmissionCategory::Reconciliation)
            .await;
        let Some(loaded) = self.load(&id.process_id).await? else {
            return Err(ActError::Runtime(format!(
                "aggregate {} not found",
                id.process_id
            )));
        };
        let record = loaded
            .snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("operation {id:?} not found")))?;
        if !matches!(record.state, OperationState::EffectUnknown { .. }) {
            return Err(ActError::Runtime(format!(
                "operation {id:?} is {:?}, not an unknown effect",
                record.state
            )));
        }
        match resolution {
            crate::engine::EffectResolution::Applied { outputs } => {
                let applied = self
                    .converge_observed_effect(
                        id,
                        effects::EffectObservation::Applied(effects::EffectOutcome::from_outputs(
                            outputs,
                        )),
                    )
                    .await?;
                if !applied {
                    return Err(ActError::Runtime(format!(
                        "operation {id:?} is no longer reconcilable"
                    )));
                }
                Ok(())
            }
            crate::engine::EffectResolution::NotApplied => {
                if !self.retry_unknown_effect(&record).await? {
                    return Err(ActError::Runtime(format!(
                        "operation {id:?} is no longer reconcilable"
                    )));
                }
                Ok(())
            }
        }
    }

    /// One driver tick for the test suites. The production driver runs the
    /// same four passes on independent intervals (A10); this helper runs them
    /// once, in order, so a test that calls it observes every loop.
    #[cfg(test)]
    pub(crate) async fn tick(&self, now_ms: i64) -> Result<RecoveryReport> {
        self.drive_ready(now_ms, PAGE_SIZE).await?;
        let report = self.recover(now_ms).await?;
        self.repair_projections().await;
        Ok(report)
    }

    /// Re-derive exceptional work from the recovery index.
    ///
    /// This method is only the IO shell around
    /// [`recover_operation`](crate::scheduler::core::recovery::planner::recover_operation)
    /// (A7): the paging walk, one [`Observed`] per row, and the directive the
    /// pure decision names. Normal `Pending`/`RetryScheduled` operations are
    /// not woken here: they own a durable ready record and the driver's due
    /// pass is their liveness source. Recovery handles what that path cannot:
    /// an expired claim is reclaimed, a committed-but-unfinalized operation is
    /// completed through the normal post-commit program, effect-unknown and
    /// quarantined operations follow the executor's proof or go to a human,
    /// and a missing ready record (an older database, a damaged index) is
    /// repaired.
    #[cfg(test)]
    pub(crate) async fn recover(&self, now_ms: i64) -> Result<RecoveryReport> {
        self.recover_with_budget(now_ms, PAGE_SIZE).await
    }

    /// The recovery scanner's own pass (A10). The scan resumes at the last
    /// examined index entry when `budget` is reached, so a large exceptional
    /// backlog is bounded work per round; an in-memory cursor is only a
    /// scheduling hint and restarts from the beginning after a restart.
    ///
    /// Projection repair is **not** part of this pass: it has its own loop
    /// ([`Self::repair_projections_batch`]) and its own interval, so changing
    /// the recovery cadence cannot change projection lag and vice versa.
    pub(crate) async fn recover_with_budget(
        &self,
        now_ms: i64,
        budget: usize,
    ) -> Result<RecoveryReport> {
        self.ensure_boot(now_ms).await?;
        let budget = budget.max(1);
        let _recovery = self.enter_admission(AdmissionCategory::Recovery).await;
        let mut report = RecoveryReport::default();
        let mut cursor = self.recovery_cursor.lock().clone();
        let mut more_pending = false;
        loop {
            let page = self
                .recovery
                .recoverable_page(cursor.as_ref(), PAGE_SIZE.min(budget))
                .await
                .map_err(runtime)?;
            if page.operations.is_empty() && page.next.is_none() {
                cursor = None;
                break;
            }
            for candidate in page.operations {
                if report.scanned >= budget {
                    more_pending = true;
                    break;
                }
                let id = candidate.operation.id.clone();
                let process_key = id.process_id.clone();
                report.scanned += 1;
                self.recover_observed(candidate, now_ms, &mut report).await;
                cursor = Some(RecoveryCursor {
                    process_key,
                    operation: Some(id),
                });
            }
            if more_pending {
                break;
            }
            match page.next {
                Some(next) => {
                    if report.scanned >= budget {
                        cursor = Some(next);
                        more_pending = true;
                        break;
                    }
                    cursor = Some(next);
                }
                None => {
                    cursor = None;
                    break;
                }
            }
        }
        *self.recovery_cursor.lock() = if more_pending { cursor } else { None };
        {
            let mut metrics = self.recovery_metrics.lock();
            metrics.scanned = report.scanned;
            metrics.more_pending = more_pending;
            metrics.passes += 1;
            metrics.reclaimed += report.reclaimed.len() as u64;
            metrics.ready_repaired += report.ready_repaired.len() as u64;
            metrics.finalized += report.finalized.len() as u64;
            metrics.effect_applied += report.effect_applied.len() as u64;
            metrics.effect_retried += report.effect_retried.len() as u64;
            metrics.reconciliation_required = report.reconciliation_required.len();
        }
        Ok(report)
    }

    /// The one-time boot passes: rebuild the derived indexes and catch the
    /// read model up from the aggregates. Both are idempotent and independent
    /// of the periodic loop that notices they are still pending (A10).
    pub(crate) async fn ensure_boot(&self, now_ms: i64) -> Result<()> {
        self.ensure_ready_boot(now_ms).await?;
        self.ensure_projection_boot().await;
        Ok(())
    }

    /// Rebuild the derived ready/recovery indexes from the authoritative
    /// aggregates once per host (A2's cold-start repair). Owned by the
    /// ready/timer loop: a recovery cadence cannot delay it and its failure is
    /// retried by the next ready pass.
    pub(crate) async fn ensure_ready_boot(&self, now_ms: i64) -> Result<()> {
        if self
            .boot_reconciled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        let repaired = self
            .processes
            .repair_all_ready(now_ms, self.timer_poll_ms)
            .await
            .map_err(runtime)?;
        self.boot_reconciled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if repaired > 0 {
            self.ready_metrics.lock().repaired += repaired as u64;
            self.ready.notify_one();
        }
        Ok(())
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

    /// One observed operation, from the observation to the executed decision.
    ///
    /// This is the IO half of [`recover_operation`]. It collects the
    /// executor's proof when the decision asks for one, commits the command
    /// Core names under the pid's command lane (the same lane and the same
    /// revision fence a normal operation uses), and runs the directive. No
    /// branch here decides what a phase means: `ReclaimReady` versus
    /// `ReclaimUnknown`, replay versus retry, and finalize are Core's answers.
    ///
    /// A command that loses its guard is left to the next scan — the state it
    /// was formed from moved on, so the new state will be observed afresh.
    async fn recover_observed(
        &self,
        candidate: RecoveryCandidate,
        now_ms: i64,
        report: &mut RecoveryReport,
    ) {
        let mut observed = Observed::from_record(&candidate.operation, candidate.version, now_ms);
        // The payload of an `Applied`/`Failed` observation, kept for the
        // `ReplayEffect` directive. Core sees only the receipt.
        let mut effect = None;
        loop {
            let decision = recover_operation(&observed);
            match decision.directive {
                RecoveryDirective::AwaitOwner => {
                    report.awaiting_owner.push(observed.operation_id);
                    return;
                }
                RecoveryDirective::IgnoreTerminal => return,
                RecoveryDirective::RepairReady => {
                    match self
                        .processes
                        .repair_ready(
                            &observed.operation_id.process_id,
                            now_ms,
                            self.timer_poll_ms,
                        )
                        .await
                    {
                        Ok(true) => {
                            self.ready_metrics.lock().repaired += 1;
                            report.ready_repaired.push(observed.operation_id);
                            self.ready.notify_one();
                        }
                        Ok(false) => {}
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "repairing the ready record failed; next scan retries"
                            );
                        }
                    }
                    return;
                }
                RecoveryDirective::ReclaimReady => {
                    let Some(command) = decision.command else {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            "reclaim decision carried no command"
                        );
                        return;
                    };
                    // A4: the reclaim is a command of the same pid — it takes
                    // the lane for the event alone. The revision guard the
                    // event carries stays the fence against a claim that moved
                    // since the scan.
                    let reclaimed = {
                        let _lane = self
                            .lock_command_lane(&observed.operation_id.process_id)
                            .await;
                        self.operation_event(
                            &observed.operation_id,
                            command.expected_revision,
                            command.event,
                        )
                        .await
                    };
                    match reclaimed {
                        Ok((reclaimed, _)) => {
                            report.reclaimed.push(reclaimed.id.clone());
                            // The event's own commit wrote the ready record;
                            // the wake is that record's accelerator, not a
                            // second business decision.
                            self.wake(&reclaimed.id);
                        }
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "reclaim lost; next scan retries"
                            );
                        }
                    }
                    return;
                }
                RecoveryDirective::ReclaimUnknown => {
                    let Some(command) = decision.command else {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            "reclaim decision carried no command"
                        );
                        return;
                    };
                    let reclaimed = {
                        let _lane = self
                            .lock_command_lane(&observed.operation_id.process_id)
                            .await;
                        self.operation_event(
                            &observed.operation_id,
                            command.expected_revision,
                            command.event,
                        )
                        .await
                    };
                    match reclaimed {
                        Ok((reclaimed, loaded)) => {
                            // The commit turned the operation `EffectUnknown`.
                            // Observe the new state and let Core decide again;
                            // the effect itself is never re-driven here.
                            observed =
                                Observed::from_record(&reclaimed, loaded.snapshot.version, now_ms);
                        }
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "reclaim lost; next scan retries"
                            );
                            return;
                        }
                    }
                }
                RecoveryDirective::FinalizeCommit => {
                    let Some(command) = decision.command else {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            "finalize decision carried no command"
                        );
                        return;
                    };
                    // A4: the close is a command of the same pid, fenced by
                    // the operation revision. The lane is dropped before the
                    // post-commit program, which may report lifecycle and
                    // deliver records.
                    let finalized = {
                        let _lane = self
                            .lock_command_lane(&observed.operation_id.process_id)
                            .await;
                        self.operation_event(
                            &observed.operation_id,
                            command.expected_revision,
                            command.event,
                        )
                        .await
                    };
                    match finalized {
                        Ok((finalized, mut loaded)) => {
                            // A6/A7: the same post-commit program a normal
                            // `dispatch` runs on the snapshot its commit
                            // returned — rows current, then lifecycle, outbox
                            // (nothing new here), then admission.
                            let admission = self.admissions.lock().directive(&loaded.snapshot);
                            let decision = Decision::recovered(&loaded.snapshot, admission);
                            if let Err(error) = self
                                .execute_decision(
                                    &mut loaded,
                                    &decision,
                                    operation::Projection::Touched,
                                    &[],
                                )
                                .await
                            {
                                tracing::warn!(
                                    operation = ?finalized.id,
                                    error = %error,
                                    "executing a finalize commit's decision failed; the recovery pass retries the rest"
                                );
                            }
                            report.finalized.push(finalized.id);
                        }
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "finalize lost; next scan retries"
                            );
                        }
                    }
                    return;
                }
                RecoveryDirective::ObserveEffect => match self.observe_effect(&observed).await {
                    Ok(Some(proof)) => {
                        effect = proof.observation;
                        observed = observed.with_effect_receipt(proof.receipt);
                    }
                    Ok(None) => {
                        report.reconciliation_required.push(observed.operation_id);
                        return;
                    }
                    Err(error) => {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            error = %error,
                            "effect observation failed; next scan retries"
                        );
                        report.reconciliation_required.push(observed.operation_id);
                        return;
                    }
                },
                RecoveryDirective::ReplayEffect => {
                    let Some(observation) = effect.take() else {
                        report.reconciliation_required.push(observed.operation_id);
                        return;
                    };
                    match self
                        .converge_observed_effect(&observed.operation_id, observation)
                        .await
                    {
                        Ok(true) => report.effect_applied.push(observed.operation_id),
                        Ok(false) => report.reconciliation_required.push(observed.operation_id),
                        Err(error) => {
                            tracing::warn!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "replaying an observed effect failed; the operation stays unknown"
                            );
                            report.reconciliation_required.push(observed.operation_id);
                        }
                    }
                    return;
                }
                RecoveryDirective::RetryEffect => {
                    match self.retry_unknown_effect(&observed.record()).await {
                        Ok(true) => report.effect_retried.push(observed.operation_id),
                        Ok(false) => report.reconciliation_required.push(observed.operation_id),
                        Err(error) => {
                            tracing::warn!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "re-queuing an idempotent effect failed; the next scan retries"
                            );
                            report.reconciliation_required.push(observed.operation_id);
                        }
                    }
                    return;
                }
                RecoveryDirective::Manual => {
                    report.reconciliation_required.push(observed.operation_id);
                    return;
                }
            }
        }
    }

    /// Deliver the records one operation just committed and persist every
    /// outcome in **one** follow-up aggregate commit. This is the normal path
    /// (A1b): the plan commit wrote the records `Pending`, the read model was
    /// projected from that commit, and only now does the transport see them.
    /// The confirmation batch is one guarded replace — never one claim and one
    /// confirm per record — so a many-record operation still pays one extra
    /// commit.
    ///
    /// A lost guard applies nothing: the records stay `Pending`, the outbox
    /// pass redelivers them (at-least-once), and the run stays unreclaimable
    /// until every record is `Delivered`.
    async fn deliver_committed_outbox(
        &self,
        loaded: &mut crate::scheduler::durable::document_store::LoadedProcess,
        ids: &[crate::scheduler::core::outbox::records::OutboxId],
    ) {
        if ids.is_empty() {
            return;
        }
        let process_id = loaded.snapshot.process_id.clone();
        let snapshot = &loaded.snapshot;
        let now = SystemClock.now_ms();
        let mut confirmations = Vec::with_capacity(ids.len());
        {
            let mut transport = self.transport.write();
            for id in ids {
                let Some(record) = snapshot.outbox.get(id) else {
                    continue;
                };
                // Only a record this operation left `Pending` is ours to
                // deliver. A concurrent pass may have claimed or settled it
                // between the commit and this pass; that worker owns it now.
                if !matches!(
                    record.state,
                    crate::scheduler::core::outbox::records::OutboxState::Pending { .. }
                ) {
                    continue;
                }
                let outcome = transport.deliver(record);
                let claim = delivery_claim(record, now);
                confirmations.push(
                    crate::scheduler::core::outbox::outbox_repository::OutboxConfirmation {
                        id: id.clone(),
                        expected_revision: record.revision,
                        events: delivery_events(&claim, outcome, now),
                    },
                );
            }
        }
        if confirmations.is_empty() {
            return;
        }
        match self
            .processes
            .confirm_outbox_batch(loaded, &confirmations)
            .await
        {
            Ok(outcome) => {
                // A rescheduled record whose retry is due now must not wait for
                // the tick; the pass is the retry net, not the normal path.
                let due_now = outcome.confirmed.iter().any(|record| {
                    matches!(
                        record.state,
                        crate::scheduler::core::outbox::records::OutboxState::Pending {
                            available_at_ms
                        } if available_at_ms <= now
                    )
                });
                if due_now {
                    self.deliver.notify_one();
                }
            }
            Err(error) => {
                tracing::warn!(
                    pid = %process_id,
                    error = %error,
                    "outbox confirmation lost; the retry pass will redeliver"
                );
                self.deliver.notify_one();
            }
        }
    }

    /// Deliver one already-loaded record: claim it with a fenced claim, hand
    /// it to the transport, then persist the reported outcome. After the
    /// transport ran, only its explicit `RetryAt` proves no external effect —
    /// everything else the transport cannot classify stays `DeliveryUnknown`.
    /// The transport lock is taken only for the synchronous `deliver` call,
    /// never across an await.
    ///
    /// `Ok(true)` means the transport confirmed delivery; `Ok(false)` means the
    /// claim was lost or the record was no longer dispatchable.
    async fn dispatch_outbox_record(&self, record: OutboxRecord, now: i64) -> Result<bool> {
        let claim = delivery_claim(&record, now);
        // A4: the claim and the outcome are two guarded aggregate writes of
        // this record's pid, so each takes the command lane — never across the
        // transport's synchronous call, exactly like an effect. The record's
        // revision stays the fence against a writer the lane cannot see.
        let lane = self.lock_command_lane(&record.id.process_id).await;
        let claimed = match self
            .processes
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
                // A concurrent worker claimed it first; skip without failing
                // the pass.
                tracing::debug!(record = ?record.id, error = %error, "delivery claim lost");
                return Ok(false);
            }
        };
        drop(lane);
        let event = {
            let outcome = self.transport.write().deliver(&claimed);
            outcome_event(&claim, outcome, now)
        };
        let delivered = matches!(&event, OutboxEvent::ConfirmDelivered { .. });
        // A4: the outcome write is a second command of the same pid — the
        // lane is retaken for it, after the transport's synchronous call.
        let _lane = self.lock_command_lane(&record.id.process_id).await;
        self.processes
            .evolve_outbox(&record.id, claimed.revision, event)
            .await
            .map_err(runtime)?;
        Ok(delivered)
    }

    /// One outbox delivery pass over the attached transport: claim every due
    /// record with a fenced claim, hand it to the transport, then persist the
    /// reported outcome.
    #[cfg(test)]
    pub(crate) async fn dispatch_outbox(&self) -> Result<usize> {
        self.dispatch_outbox_batch(PAGE_SIZE).await
    }

    /// The bounded outbox retry pass (A10). At most `budget` records are
    /// delivered per round; a larger backlog resumes at the next record on the
    /// loop's next interval. The cursor is in-memory scheduling state only —
    /// records remain `Pending` in the aggregate, so losing it (restart) just
    /// restarts the pass.
    pub(crate) async fn dispatch_outbox_batch(&self, budget: usize) -> Result<usize> {
        let now = SystemClock.now_ms();
        let budget = budget.max(1);
        let mut delivered = 0usize;
        let mut processed = 0usize;
        let mut deferred_records = 0u64;
        let mut settled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // Pids whose rows this pass verified current. The normal path already
        // projected before it delivered; this pass also serves records a
        // failed/withheld projection left `Pending` (and records another
        // instance committed), so it re-projects the run before any external
        // consumer can see the message.
        let mut projected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // Pids whose projection failed in this pass: their records stay
        // `Pending`, and retrying the projection for every record of the run
        // would only multiply a store outage.
        let mut deferred: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cursor = self.outbox_cursor.lock().clone();
        let mut last_processed = cursor.clone();
        let mut first_page = true;
        let mut wrapped = false;
        'pass: loop {
            let remaining = budget.saturating_sub(processed).max(1);
            let page = self
                .processes
                .dispatchable_outbox_page(now, cursor.as_ref(), PAGE_SIZE.min(remaining))
                .await
                .map_err(runtime)?;
            if first_page {
                first_page = false;
                let mut metrics = self.outbox_metrics.lock();
                metrics.observed = page.observed;
                metrics.backlog = page.due;
                metrics.oldest_due_age_ms = page
                    .oldest_due_at_ms
                    .map(|at| now.saturating_sub(at).max(0));
                metrics.passes += 1;
            }
            if page.records.is_empty() {
                // A cursor past the last due record only means the pass
                // wrapped around: start once from the beginning before
                // deciding the backlog is empty.
                if cursor.is_some() && !wrapped {
                    cursor = None;
                    last_processed = None;
                    wrapped = true;
                    continue;
                }
                cursor = None;
                break;
            }
            for record in page.records {
                if processed >= budget {
                    cursor = last_processed.clone();
                    break 'pass;
                }
                last_processed = Some(OutboxCursor(record.id.clone()));
                let process_id = record.id.process_id.clone();
                if deferred.contains(&process_id) {
                    processed += 1;
                    deferred_records += 1;
                    continue;
                }
                if !projected.contains(&process_id) {
                    if !self.repair_projection(&process_id).await {
                        // The rows are still behind; leave every record of
                        // this run `Pending` for the next pass.
                        deferred.insert(process_id.clone());
                        processed += 1;
                        deferred_records += 1;
                        continue;
                    }
                    projected.insert(process_id.clone());
                }
                processed += 1;
                if self.dispatch_outbox_record(record, now).await? {
                    delivered += 1;
                    settled.insert(process_id);
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        *self.outbox_cursor.lock() = cursor;
        {
            let mut metrics = self.outbox_metrics.lock();
            metrics.delivered += delivered as u64;
            metrics.deferred += deferred_records;
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

// TEMPORARY (P7 task 1a): the per-phase operation timers the profile probe reads.
// Deleted together with the probe and the §10 record they feed.
#[cfg(test)]
pub(crate) mod operation_trace {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    pub(crate) const OPERATIONS: usize = 0;
    pub(crate) const LOAD: usize = 1;
    pub(crate) const PLACE1: usize = 2;
    pub(crate) const CLAIM: usize = 3;
    pub(crate) const PLAN: usize = 4;
    pub(crate) const DELIVER: usize = 5;
    pub(crate) const COMMIT: usize = 6;
    pub(crate) const WAKE: usize = 7;
    pub(crate) const LIFECYCLE: usize = 8;
    pub(crate) const PROJECT: usize = 9;
    pub(crate) const PLACE2: usize = 10;
    pub(crate) const STAGES: usize = 11;
    pub(crate) const NAMES: [&str; STAGES] = [
        "operations",
        "load",
        "place1",
        "claim",
        "plan",
        "deliver",
        "commit",
        "wake",
        "lifecycle",
        "project",
        "place2",
    ];

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static TOTALS: parking_lot::Mutex<[u64; STAGES]> = parking_lot::Mutex::new([0; STAGES]);

    pub(crate) fn enable() {
        *TOTALS.lock() = [0; STAGES];
        ENABLED.store(true, Ordering::SeqCst);
    }

    pub(crate) fn disable() -> [u64; STAGES] {
        ENABLED.store(false, Ordering::SeqCst);
        *TOTALS.lock()
    }

    pub(crate) fn operation() {
        if ENABLED.load(Ordering::Relaxed) {
            TOTALS.lock()[OPERATIONS] += 1;
        }
    }

    pub(crate) fn start() -> Option<Instant> {
        ENABLED.load(Ordering::Relaxed).then(Instant::now)
    }

    pub(crate) fn end(stage: usize, started: Option<Instant>) {
        if let Some(started) = started {
            TOTALS.lock()[stage] += started.elapsed().as_nanos() as u64;
        }
    }
}

#[cfg(not(test))]
pub(crate) mod operation_trace {
    use std::time::Instant;

    pub(crate) const LOAD: usize = 1;
    pub(crate) const PLACE1: usize = 2;
    pub(crate) const CLAIM: usize = 3;
    pub(crate) const PLAN: usize = 4;
    pub(crate) const DELIVER: usize = 5;
    pub(crate) const COMMIT: usize = 6;
    pub(crate) const WAKE: usize = 7;
    pub(crate) const LIFECYCLE: usize = 8;
    pub(crate) const PROJECT: usize = 9;
    pub(crate) const PLACE2: usize = 10;

    pub(crate) fn operation() {}
    pub(crate) fn start() -> Option<Instant> {
        None
    }
    pub(crate) fn end(_stage: usize, _started: Option<Instant>) {}
}
