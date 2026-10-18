//! The complete decision a committed transition implies.
//!
//! A [`TransitionPlan`] says what the aggregate commit writes: task creates,
//! state changes, variable writes, the operation it closes, the operations it
//! creates and the outbox records it makes durable. It does **not** say what
//! that commit means for the run as a whole. That second half — is the run
//! still in flight, may its slot be freed, which lifecycle event does the
//! snapshot imply, may a parked run take its place — used to be re-derived,
//! in subtly different forms, inside the shell's dispatch, lifecycle, admission
//! and projection paths (A6).
//!
//! This module is the one place those rules live. Everything here is pure: it
//! reads a [`ProcessSnapshot`] and values the caller observed (the driver's
//! in-memory slot set), and returns values. Nothing here performs IO, awaits,
//! or reaches a layer above `core`.
//!
//! # The two settlement views
//!
//! - [`run_settled`] — every execution is terminal. The run is no longer in
//!   flight, so its admission slot may be released and its lifecycle may be
//!   reported. It deliberately ignores outbox and operations.
//! - [`projection_settled`] — [`ProcessSnapshot::is_reclaimable`]: every
//!   execution terminal **and** every outbox record delivered **and** every
//!   operation retired. Only then may the rows and the durable aggregate be
//!   reclaimed (A11).
//!
//! Both are carried together in [`Settlement`] so a shell can read the one it
//! needs without re-deriving either.
//!
//! # Admission categories
//!
//! A run that is over — or one whose only remaining route is manual
//! reconciliation — does not occupy an execution slot. The other budgets
//! (concurrent effects, recovery, human reconciliation and projection) are
//! independent ([`AdmissionCategory`], [`quota`]), so a saturated category
//! never holds another category's work. Execution's richer rule is [`place`];
//! the shell owns the sets and executes the directives.

use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::{
        operation::{OperationId, OperationState},
        task_graph::{TaskId, TaskState},
        transition_plan::TransitionPlan,
    },
    plan::{
        ActPlan as _,
        concurrency::{Admission as SlotAdmission, Admit as SlotPolicy},
        infallible,
    },
};

/// The operation that admits a run the driver did not seed itself — one
/// already in flight when it booted: the seed's root `ExecuteTask` (sequence 0
/// of the root execution), which is the operation that starts a `Created` root.
/// A run whose root operation already ran has no such operation left, so
/// admitting it is a no-op; its real work already owns a ready record.
pub(crate) fn resume_operation(snapshot: &ProcessSnapshot) -> OperationId {
    let root = root_task(snapshot).unwrap_or_else(|| {
        TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0)
            .expect("the root tid constant is a valid node id")
    });
    OperationId {
        process_id: snapshot.process_id.clone(),
        task: root,
        sequence: 0,
    }
}

/// The whole-run status implied by the root execution's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessStatus {
    /// The root has not started (`Created`/`Ready`, or no seed at all).
    Pending,
    /// The root is in flight (`Running`/`Waiting`).
    Running,
    /// The root completed (`Completed`/`Submitted`/`Backed`).
    Completed,
    /// The root failed.
    Failed,
    /// The root reached some other terminal state (`Aborted`, `Cancelled`,
    /// `Skipped`, `Removed`).
    Terminated,
}

impl ProcessStatus {
    pub(crate) fn of(snapshot: &ProcessSnapshot) -> Self {
        let Some(root) = root_task(snapshot) else {
            return Self::Pending;
        };
        match snapshot.tasks.get(&root).map(|task| task.state) {
            Some(TaskState::Created | TaskState::Ready) => Self::Pending,
            Some(TaskState::Running | TaskState::Waiting) => Self::Running,
            Some(TaskState::Completed | TaskState::Submitted | TaskState::Backed) => {
                Self::Completed
            }
            Some(TaskState::Failed) => Self::Failed,
            Some(
                TaskState::Cancelled | TaskState::Aborted | TaskState::Skipped | TaskState::Removed,
            ) => Self::Terminated,
            None => Self::Pending,
        }
    }
}

/// The lifecycle event one snapshot implies, once and only once per phase.
///
/// `None` for a snapshot with no root: nothing has started, so nothing is
/// reportable. The shell deduplicates events per pid; this type only says
/// which event the committed state means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LifecycleDirective {
    /// The run is alive and its start has not been reported.
    Started,
    /// Every execution settled and the root is not failed.
    Completed,
    /// Every execution settled and the root failed; the root's durable error
    /// facts are carried so the shell does not re-read them.
    Errored { code: String, message: String },
}

/// The two settlement views (see the module docs).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Settlement {
    /// [`run_settled`]: the run holds no admission slot and reports lifecycle.
    pub(crate) run: bool,
    /// [`projection_settled`]: the run's rows and aggregate may be reclaimed.
    pub(crate) projection: bool,
}

/// Whether every execution reached a terminal state: the run is no longer in
/// flight, so the slot it holds is free and its terminal lifecycle may be
/// reported. Deliberately weaker than [`projection_settled`], which also waits
/// for the outbox and outstanding operations; those keep the run's rows alive,
/// never its slot.
pub(crate) fn run_settled(snapshot: &ProcessSnapshot) -> bool {
    !snapshot.tasks.is_empty() && snapshot.tasks.values().all(|task| task.state.is_terminal())
}

/// Whether the run's remaining route is manual reconciliation (A8): at least
/// one operation crossed the external-effect boundary without a provable
/// outcome (`EffectUnknown`) or was explicitly quarantined.
///
/// Such a run is still in flight, but ordinary admission cannot advance it:
/// only the reconciliation path may. It therefore does not occupy an execution
/// slot; the reconciliation quota bounds the human work instead.
pub(crate) fn awaiting_reconciliation(snapshot: &ProcessSnapshot) -> bool {
    snapshot.operations.values().any(|operation| {
        matches!(
            operation.state,
            OperationState::EffectUnknown { .. } | OperationState::Quarantined { .. }
        )
    })
}

/// Whether the read model and the durable state may be reclaimed (A11):
/// terminal executions, delivered outbox records, retired operations. This is
/// the aggregate's own rule; the shell's sweeper only executes it.
pub(crate) fn projection_settled(snapshot: &ProcessSnapshot) -> bool {
    snapshot.is_reclaimable()
}

/// Both settlement views, derived once.
pub(crate) fn settle(snapshot: &ProcessSnapshot) -> Settlement {
    Settlement {
        run: run_settled(snapshot),
        projection: projection_settled(snapshot),
    }
}

/// The run's lifecycle event from the committed snapshot.
pub(crate) fn lifecycle(snapshot: &ProcessSnapshot) -> Option<LifecycleDirective> {
    let root = root_task(snapshot)?;
    if !run_settled(snapshot) {
        return Some(LifecycleDirective::Started);
    }
    if snapshot.tasks.get(&root)?.state == TaskState::Failed {
        let (code, message) = root_error(snapshot, &root);
        return Some(LifecycleDirective::Errored { code, message });
    }
    Some(LifecycleDirective::Completed)
}

/// The terminal outcome a settled run reports to its [`RunHandle`].
///
/// Derived from the same facts as [`lifecycle`]: a failed root carries the
/// durable `error.code`/`error.message` the upward walk copied into its scope;
/// a terminal root that is neither completed nor failed (aborted, cancelled,
/// skipped, removed) is `Terminated`. `None` while the run is still in flight.
///
/// [`RunHandle`]: crate::RunHandle
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// Every execution settled and the root completed.
    Completed,
    /// Every execution settled and the root failed.
    Errored { code: String, message: String },
    /// Every execution settled and the root reached another terminal state.
    Terminated,
}

/// The run's terminal outcome, or `None` while it is still in flight.
///
/// The one rule a durable wait needs: [`run_settled`] first (the same
/// settlement [`LifecycleDirective`] reports), then the root's own facts. A
/// waiter never re-derives a state machine from operations or rows.
pub(crate) fn run_outcome(snapshot: &ProcessSnapshot) -> Option<RunOutcome> {
    if !run_settled(snapshot) {
        return None;
    }
    match lifecycle(snapshot) {
        Some(LifecycleDirective::Errored { code, message }) => {
            Some(RunOutcome::Errored { code, message })
        }
        _ if ProcessStatus::of(snapshot) == ProcessStatus::Terminated => {
            Some(RunOutcome::Terminated)
        }
        _ => Some(RunOutcome::Completed),
    }
}

/// The root execution of an aggregate: the `$` node's earliest generation —
/// the generation a re-entry never removes.
pub(crate) fn root_task(snapshot: &ProcessSnapshot) -> Option<TaskId> {
    snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == crate::utils::consts::TASK_ROOT_TID)
        .min_by_key(|id| id.generation)
        .cloned()
}

/// The root execution's state — what the slot rule bounds. A snapshot without
/// a root reads as running, so the run asks for a slot instead of being
/// admitted for free.
pub(crate) fn root_state(snapshot: &ProcessSnapshot) -> TaskState {
    root_task(snapshot)
        .and_then(|root| snapshot.tasks.get(&root).map(|task| task.state))
        .unwrap_or(TaskState::Running)
}

/// The failed root's error variables (`error.code` / `error.message`), in the
/// text form the lifecycle message carries.
fn root_error(snapshot: &ProcessSnapshot, root: &TaskId) -> (String, String) {
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(root.clone(), name.to_string()))
            .map(|raw| json_text(raw))
            .unwrap_or_default()
    };
    (read("error.code"), read("error.message"))
}

/// One scope variable that holds a JSON string, decoded (`raw` when it is not
/// a JSON string).
fn json_text(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string())
}

/// Where a run stands in the driver's in-memory slot set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SlotStanding {
    /// Waiting in the parked queue.
    Parked,
    /// Holding one of the `cap` execution slots.
    Holding,
    /// Not admitted by this driver — a run already in flight when it booted.
    Unplaced,
}

/// What the driver must do with the run's slot before it drives an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SlotDirective {
    /// Keep the slot the run holds.
    Keep,
    /// The run no longer needs the execution slot — every execution settled,
    /// or it is waiting on manual reconciliation: free its slot, then drive
    /// (its own remaining work, a timer operation for instance, needs no
    /// slot).
    Release,
    /// Take a slot, then drive.
    Take,
    /// The root is already terminal: drive without taking a slot.
    NoSlot,
}

/// The admission directive a slot observation implies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionDirective {
    /// Drive the operation with the named slot action.
    Drive(SlotDirective),
    /// Hold the operation back until a slot frees.
    Park,
}

impl AdmissionDirective {
    /// Whether the operation may be driven now.
    pub(crate) fn drives(self) -> bool {
        matches!(self, Self::Drive(_))
    }
}

/// The driver-owned facts the admission policy reads. The slot set itself is
/// in-memory shell state; the rule is here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SlotObservation {
    pub(crate) standing: SlotStanding,
    pub(crate) cap: usize,
    pub(crate) active: usize,
}

/// The run-level facts admission reads from the committed aggregate. They are
/// derived together so every category sees one answer:
///
/// - the root state bounds the ordinary slot;
/// - [`run_settled`] says the run is over;
/// - [`awaiting_reconciliation`] says ordinary execution cannot advance it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunFacts {
    pub(crate) root_state: TaskState,
    pub(crate) run_settled: bool,
    pub(crate) awaiting_reconciliation: bool,
}

impl RunFacts {
    pub(crate) fn of(snapshot: &ProcessSnapshot) -> Self {
        Self {
            root_state: root_state(snapshot),
            run_settled: run_settled(snapshot),
            awaiting_reconciliation: awaiting_reconciliation(snapshot),
        }
    }

    /// Whether the run still needs one of the execution slots: it has live
    /// business work and no manual-reconciliation hold. A quarantined run
    /// leaves the execution quota immediately, so the parked run it held back
    /// starts now; its eventual human decision is admitted by the
    /// reconciliation quota instead.
    pub(crate) fn execution_active(self) -> bool {
        !self.run_settled && !self.awaiting_reconciliation
    }
}

/// The independent admission categories (A8). Execution owns the run slots;
/// the other four bound their own work, so one category's saturation never
/// consumes another's budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionCategory {
    /// Normal runs in flight.
    Execution,
    /// Concurrent external effects.
    Effect,
    /// Recovery scans and their exceptional commands.
    Recovery,
    /// Human decisions on `EffectUnknown`/`Quarantined` operations.
    Reconciliation,
    /// Row projections and their repair queue.
    Projection,
}

/// Whether one more holder of an auxiliary quota may start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuotaDirective {
    Admit,
    Wait,
}

/// The auxiliary categories' whole rule: `active < cap`, with `cap == 0`
/// meaning unbounded. Execution's richer rule is [`place`]; this is the one
/// the effect/recovery/reconciliation/projection gates execute.
pub(crate) fn quota(cap: usize, active: usize) -> QuotaDirective {
    if cap == 0 || active < cap {
        QuotaDirective::Admit
    } else {
        QuotaDirective::Wait
    }
}

/// The admission decision for one run:
///
/// - a parked run waits, whatever its state;
/// - a run holding a slot releases it once its execution work is over —
///   every execution settled, or the run is waiting on manual reconciliation;
///   otherwise it keeps it;
/// - a run this driver never admitted takes a slot, or parks exactly like a
///   start that found every slot taken. A run whose root is already terminal
///   is admitted *because* it needs no slot — taking one here would hold it
///   for a run that never operates again. A quarantined run is likewise
///   admitted without an execution slot: only its reconciliation path may
///   advance it, and that path is bounded separately.
pub(crate) fn place(observation: SlotObservation, facts: RunFacts) -> AdmissionDirective {
    match observation.standing {
        SlotStanding::Parked => AdmissionDirective::Park,
        SlotStanding::Holding if !facts.execution_active() => {
            AdmissionDirective::Drive(SlotDirective::Release)
        }
        SlotStanding::Holding => AdmissionDirective::Drive(SlotDirective::Keep),
        SlotStanding::Unplaced => {
            if facts.root_state.is_terminal() || facts.awaiting_reconciliation {
                return AdmissionDirective::Drive(SlotDirective::NoSlot);
            }
            let admission = infallible(SlotPolicy::plan(
                (),
                &(observation.cap, observation.active, facts.root_state),
            ));
            match admission {
                SlotAdmission::Admit => AdmissionDirective::Drive(SlotDirective::Take),
                SlotAdmission::Park => AdmissionDirective::Park,
            }
        }
    }
}

/// The complete output of one core decision: the durable transition plus the
/// process-level directives the shell executes.
///
/// The design's vocabulary maps onto the plan fields without copying them:
///
/// - **aggregate changes** — `transition.task_creates` + `state_changes` +
///   `variable_writes`;
/// - **operations to create / timers / effect intents** —
///   `transition.operations`; a timer is the `DispatchTimeout` kind (its
///   `ready_at_ms` is the due instant) and an effect intent is the
///   `ApplyAction` kind. Effects are still executed by their own operation
///   before its commit (A1a/A1c); the decision never carries an effect across
///   the lane boundary (A4).
/// - **operations to close** — `transition.source_operation`, the claimed
///   operation the same atomic batch retires;
/// - **outbox records** — `transition.outbox`.
///
/// `process_status`, `settlement`, `lifecycle` and `admission` are the second
/// half: the rules that used to live in the shell's dispatch order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Decision {
    /// The durable transition, already validated by `commit_transition`.
    pub(crate) transition: TransitionPlan,
    /// The run's whole-run status after the commit.
    pub(crate) process_status: ProcessStatus,
    /// Both settlement views after the commit.
    pub(crate) settlement: Settlement,
    /// The lifecycle event the committed snapshot implies.
    pub(crate) lifecycle: Option<LifecycleDirective>,
    /// Whether the run is waiting on manual reconciliation after this commit
    /// (`EffectUnknown`/`Quarantined`). Such a run is outside the execution
    /// quota; the shell reports it on the reconciliation gate.
    pub(crate) quarantined: bool,
    /// What the driver must do with the run's execution slot.
    pub(crate) admission: AdmissionDirective,
}

impl Decision {
    /// The decision a committed snapshot implies. `admission` is computed by
    /// [`place`] from the driver's observed slot set; everything else is read
    /// from the snapshot the commit returned.
    pub(crate) fn committed(
        transition: TransitionPlan,
        snapshot: &ProcessSnapshot,
        admission: AdmissionDirective,
    ) -> Self {
        Self {
            transition,
            process_status: ProcessStatus::of(snapshot),
            settlement: settle(snapshot),
            lifecycle: lifecycle(snapshot),
            quarantined: awaiting_reconciliation(snapshot),
            admission,
        }
    }

    /// The decision a recovery commit implies (A7). Recovery changes only the
    /// operation lifecycle, never the aggregate's business state, so the
    /// transition is the empty plan at the committed version: it is never
    /// committed, and the project step it feeds touches no execution row. The
    /// process-level half is [`Self::committed`]'s — the same state yields the
    /// same decision on the normal and the recovery path.
    pub(crate) fn recovered(snapshot: &ProcessSnapshot, admission: AdmissionDirective) -> Self {
        let transition = TransitionPlan {
            process_id: snapshot.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        Self::committed(transition, snapshot, admission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::{
        commit::aggregate::TaskSnapshot,
        model::{model_graph::NodeId, transition_plan::ProcessVersion},
        outbox::records::{OutboxId, OutboxRecord, OutboxState},
    };
    use std::collections::{BTreeMap, HashMap};

    fn root() -> TaskId {
        TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap()
    }

    fn snapshot(root_state: TaskState) -> ProcessSnapshot {
        let task = root();
        ProcessSnapshot {
            process_id: "p".into(),
            version: ProcessVersion {
                graph: 0,
                process: 0,
                operations: 0,
            },
            tasks: HashMap::from([(
                task.clone(),
                TaskSnapshot {
                    model_node: NodeId::parse(crate::utils::consts::TASK_ROOT_TID).unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: root_state,
                    revision: 0,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }

    fn delivered(snapshot: &mut ProcessSnapshot) {
        for record in snapshot.outbox.values_mut() {
            record.state = OutboxState::Delivered {
                receipt: "r".into(),
            };
        }
    }

    #[test]
    fn run_and_projection_settlement_are_different_questions() {
        let mut snapshot = snapshot(TaskState::Running);
        let pending_id = OutboxId {
            process_id: snapshot.process_id.clone(),
            key: "pending".into(),
        };
        snapshot.outbox.insert(
            pending_id,
            OutboxRecord::pending(
                OutboxId {
                    process_id: snapshot.process_id.clone(),
                    key: "pending".into(),
                },
                "task.completed".into(),
                "{}".into(),
                0,
            )
            .unwrap(),
        );
        assert!(!run_settled(&snapshot));
        assert!(!projection_settled(&snapshot));

        snapshot.tasks.get_mut(&root()).unwrap().state = TaskState::Completed;
        assert!(run_settled(&snapshot), "terminal executions free the slot");
        assert!(
            !projection_settled(&snapshot),
            "a pending outbox record keeps the rows and the aggregate alive"
        );

        delivered(&mut snapshot);
        assert!(
            projection_settled(&snapshot),
            "terminal tasks + delivered outbox + no operation is reclaimable"
        );
        assert_eq!(
            settle(&snapshot),
            Settlement {
                run: true,
                projection: true
            }
        );
    }

    #[test]
    fn an_empty_snapshot_is_neither_settled() {
        let mut snapshot = snapshot(TaskState::Running);
        snapshot.tasks.clear();
        assert!(!run_settled(&snapshot));
        assert!(!projection_settled(&snapshot));
        assert_eq!(ProcessStatus::of(&snapshot), ProcessStatus::Pending);
        assert_eq!(lifecycle(&snapshot), None);
    }

    #[test]
    fn status_and_lifecycle_follow_the_root_state() {
        assert_eq!(
            ProcessStatus::of(&snapshot(TaskState::Created)),
            ProcessStatus::Pending
        );
        assert_eq!(
            lifecycle(&snapshot(TaskState::Ready)),
            Some(LifecycleDirective::Started)
        );
        assert_eq!(
            ProcessStatus::of(&snapshot(TaskState::Waiting)),
            ProcessStatus::Running
        );
        assert_eq!(
            lifecycle(&snapshot(TaskState::Running)),
            Some(LifecycleDirective::Started)
        );
        assert_eq!(
            ProcessStatus::of(&snapshot(TaskState::Completed)),
            ProcessStatus::Completed
        );
        assert_eq!(
            lifecycle(&snapshot(TaskState::Completed)),
            Some(LifecycleDirective::Completed)
        );
        assert_eq!(
            ProcessStatus::of(&snapshot(TaskState::Aborted)),
            ProcessStatus::Terminated
        );
        assert_eq!(
            lifecycle(&snapshot(TaskState::Aborted)),
            Some(LifecycleDirective::Completed),
            "a non-failed terminal root is a completion, exactly like the legacy on_proc handler"
        );
    }

    #[test]
    fn a_failed_root_reports_its_durable_error_facts() {
        let mut snapshot = snapshot(TaskState::Failed);
        let root = root();
        snapshot.variables.insert(
            (root.clone(), "error.code".into()),
            serde_json::Value::String("E42".into()).to_string(),
        );
        snapshot.variables.insert(
            (root, "error.message".into()),
            serde_json::Value::String("broken".into()).to_string(),
        );
        assert_eq!(ProcessStatus::of(&snapshot), ProcessStatus::Failed);
        assert_eq!(
            lifecycle(&snapshot),
            Some(LifecycleDirective::Errored {
                code: "E42".into(),
                message: "broken".into()
            })
        );
    }

    fn facts(root_state: TaskState, run_settled: bool) -> RunFacts {
        RunFacts {
            root_state,
            run_settled,
            awaiting_reconciliation: false,
        }
    }

    fn quarantined(root_state: TaskState) -> RunFacts {
        RunFacts {
            root_state,
            run_settled: false,
            awaiting_reconciliation: true,
        }
    }

    #[test]
    fn admission_distinguishes_parked_held_and_unplaced_runs() {
        let running = SlotObservation {
            standing: SlotStanding::Parked,
            cap: 2,
            active: 0,
        };
        assert_eq!(
            place(running, facts(TaskState::Running, false)),
            AdmissionDirective::Park,
            "a parked run waits even when a slot is free; release admits it by pid"
        );

        let held = SlotObservation {
            standing: SlotStanding::Holding,
            cap: 2,
            active: 2,
        };
        assert_eq!(
            place(held, facts(TaskState::Running, false)),
            AdmissionDirective::Drive(SlotDirective::Keep)
        );
        assert_eq!(
            place(held, facts(TaskState::Completed, true)),
            AdmissionDirective::Drive(SlotDirective::Release),
            "a settled run frees the slot that admits the oldest parked run"
        );
        assert_eq!(
            place(held, facts(TaskState::Cancelled, true)),
            AdmissionDirective::Drive(SlotDirective::Release),
            "a cancellation's deciding command frees the slot immediately"
        );

        let unplaced = SlotObservation {
            standing: SlotStanding::Unplaced,
            cap: 1,
            active: 1,
        };
        assert_eq!(
            place(unplaced, facts(TaskState::Running, false)),
            AdmissionDirective::Park,
            "a booted run meeting a full cap parks like a start that found it full"
        );
        let free = SlotObservation {
            standing: SlotStanding::Unplaced,
            cap: 1,
            active: 0,
        };
        assert_eq!(
            place(free, facts(TaskState::Running, false)),
            AdmissionDirective::Drive(SlotDirective::Take)
        );
        assert_eq!(
            place(free, facts(TaskState::Completed, true)),
            AdmissionDirective::Drive(SlotDirective::NoSlot),
            "a finished run needs no slot for its remaining bookkeeping"
        );
    }

    #[test]
    fn a_quarantined_run_leaves_the_execution_quota() {
        let held = SlotObservation {
            standing: SlotStanding::Holding,
            cap: 1,
            active: 1,
        };
        assert_eq!(
            place(held, quarantined(TaskState::Running)),
            AdmissionDirective::Drive(SlotDirective::Release),
            "an EffectUnknown/Quarantined run must free the slot it cannot use"
        );
        let unplaced = SlotObservation {
            standing: SlotStanding::Unplaced,
            cap: 1,
            active: 1,
        };
        assert_eq!(
            place(unplaced, quarantined(TaskState::Running)),
            AdmissionDirective::Drive(SlotDirective::NoSlot),
            "a full execution quota does not stop a manual reconciliation's run"
        );
        assert!(
            !quarantined(TaskState::Running).execution_active(),
            "a quarantined run is not execution work"
        );
        assert!(facts(TaskState::Running, false).execution_active());
    }

    #[test]
    fn awaiting_reconciliation_reads_both_manual_states() {
        let mut snapshot = snapshot(TaskState::Running);
        let id = OperationId {
            process_id: snapshot.process_id.clone(),
            task: root(),
            sequence: 1,
        };
        assert!(!awaiting_reconciliation(&snapshot));
        snapshot.operations.insert(
            id.clone(),
            crate::scheduler::core::model::operation::OperationRecord {
                id: id.clone(),
                kind: crate::scheduler::core::model::operation::OperationKind::ApplyAction,
                state: OperationState::EffectUnknown { attempt: 1 },
                revision: 1,
            },
        );
        assert!(awaiting_reconciliation(&snapshot));
        snapshot.operations.get_mut(&id).unwrap().state = OperationState::Quarantined {
            reason: "operator hold".into(),
        };
        assert!(awaiting_reconciliation(&snapshot));
    }

    #[test]
    fn auxiliary_quotas_are_independent() {
        assert_eq!(quota(0, 0), QuotaDirective::Admit, "zero is unbounded");
        assert_eq!(quota(4, 3), QuotaDirective::Admit);
        assert_eq!(quota(4, 4), QuotaDirective::Wait);
        assert_eq!(quota(4, 9), QuotaDirective::Wait);
    }

    #[test]
    fn normal_and_recovery_commits_of_one_state_decide_alike() {
        for state in [
            TaskState::Created,
            TaskState::Ready,
            TaskState::Running,
            TaskState::Waiting,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Aborted,
            TaskState::Cancelled,
        ] {
            let snapshot = snapshot(state);
            let admission = AdmissionDirective::Drive(SlotDirective::Keep);
            let normal = Decision::committed(
                TransitionPlan {
                    process_id: snapshot.process_id.clone(),
                    expected: snapshot.version.clone(),
                    task_creates: vec![],
                    state_changes: vec![],
                    variable_writes: vec![],
                    source_operation: None,
                    operations: vec![],
                    outbox: vec![],
                },
                &snapshot,
                admission,
            );
            let recovered = Decision::recovered(&snapshot, admission);
            assert_eq!(recovered.process_status, normal.process_status);
            assert_eq!(recovered.settlement, normal.settlement);
            assert_eq!(recovered.lifecycle, normal.lifecycle);
            assert_eq!(recovered.admission, normal.admission);
            assert_eq!(
                recovered.transition.expected, snapshot.version,
                "the empty plan names the committed version it must never write"
            );
            assert_eq!(recovered.transition.process_id, snapshot.process_id);
            assert!(recovered.transition.state_changes.is_empty());
            assert!(recovered.transition.task_creates.is_empty());
            assert!(recovered.transition.variable_writes.is_empty());
            assert!(recovered.transition.operations.is_empty());
            assert!(recovered.transition.outbox.is_empty());
            assert_eq!(recovered.transition.source_operation, None);
        }
    }

    #[test]
    fn the_decision_carries_the_plan_and_every_process_directive() {
        let snapshot = snapshot(TaskState::Completed);
        let transition = TransitionPlan {
            process_id: snapshot.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: vec![],
            state_changes: vec![],
            variable_writes: vec![],
            source_operation: None,
            operations: vec![],
            outbox: vec![],
        };
        let decision = Decision::committed(
            transition.clone(),
            &snapshot,
            AdmissionDirective::Drive(SlotDirective::Release),
        );
        assert_eq!(decision.transition, transition);
        assert_eq!(decision.process_status, ProcessStatus::Completed);
        assert_eq!(
            decision.settlement,
            Settlement {
                run: true,
                projection: true
            },
            "terminal tasks with no outbox and no outstanding operation are reclaimable"
        );
        assert_eq!(decision.lifecycle, Some(LifecycleDirective::Completed));
        assert_eq!(
            decision.admission,
            AdmissionDirective::Drive(SlotDirective::Release)
        );
    }
}
