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

use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::{
        operation::OperationId,
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
    /// The run settled: free its slot, then drive (its own remaining work, a
    /// timer operation for instance, needs no slot).
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

/// The admission decision for one run:
///
/// - a parked run waits, whatever its state;
/// - a run holding a slot releases it once every execution settled, otherwise
///   keeps it;
/// - a run this driver never admitted takes a slot, or parks exactly like a
///   start that found every slot taken. A run whose root is already terminal
///   is admitted *because* it needs no slot — taking one here would hold it
///   for a run that never operates again.
pub(crate) fn place(
    observation: SlotObservation,
    root_state: TaskState,
    run_settled: bool,
) -> AdmissionDirective {
    match observation.standing {
        SlotStanding::Parked => AdmissionDirective::Park,
        SlotStanding::Holding if run_settled => AdmissionDirective::Drive(SlotDirective::Release),
        SlotStanding::Holding => AdmissionDirective::Drive(SlotDirective::Keep),
        SlotStanding::Unplaced => {
            let admission = infallible(SlotPolicy::plan(
                (),
                &(observation.cap, observation.active, root_state),
            ));
            match admission {
                SlotAdmission::Admit if root_state.is_terminal() => {
                    AdmissionDirective::Drive(SlotDirective::NoSlot)
                }
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
    /// What the driver must do with the run's slot.
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
            admission,
        }
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

    #[test]
    fn admission_distinguishes_parked_held_and_unplaced_runs() {
        let running = SlotObservation {
            standing: SlotStanding::Parked,
            cap: 2,
            active: 0,
        };
        assert_eq!(
            place(running, TaskState::Running, false),
            AdmissionDirective::Park,
            "a parked run waits even when a slot is free; release admits it by pid"
        );

        let held = SlotObservation {
            standing: SlotStanding::Holding,
            cap: 2,
            active: 2,
        };
        assert_eq!(
            place(held, TaskState::Running, false),
            AdmissionDirective::Drive(SlotDirective::Keep)
        );
        assert_eq!(
            place(held, TaskState::Completed, true),
            AdmissionDirective::Drive(SlotDirective::Release),
            "a settled run frees the slot that admits the oldest parked run"
        );

        let unplaced = SlotObservation {
            standing: SlotStanding::Unplaced,
            cap: 1,
            active: 1,
        };
        assert_eq!(
            place(unplaced, TaskState::Running, false),
            AdmissionDirective::Park,
            "a booted run meeting a full cap parks like a start that found it full"
        );
        let free = SlotObservation {
            standing: SlotStanding::Unplaced,
            cap: 1,
            active: 0,
        };
        assert_eq!(
            place(free, TaskState::Running, false),
            AdmissionDirective::Drive(SlotDirective::Take)
        );
        assert_eq!(
            place(free, TaskState::Completed, true),
            AdmissionDirective::Drive(SlotDirective::NoSlot),
            "a finished run needs no slot for its remaining bookkeeping"
        );
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
