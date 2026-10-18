//! The legacy read model, projected from the aggregate.
//!
//! The kernel's only durable truth is the aggregate document; the engine's
//! read APIs (`proc:get`/`task:get`/`msg:ls`, `Runtime::proc`) read the three
//! legacy row collections (`procs`, `tasks`, `vars`). This module materializes
//! those rows from a committed snapshot — idempotently, after every commit —
//! so the read model follows the run without ever becoming a second source of
//! truth:
//!
//! - one `procs` row per process: the root execution's state (the legacy
//!   process state), the run's model, timings and the process env
//!   (`__owner`, `__workdir`, and the package-written env map `__env`);
//! - one `tasks` row per model node — the *newest* generation, which is the
//!   surviving instance of a re-entered node — carrying the node's own
//!   `node_data` (built from the declaring model exactly like the legacy tree
//!   builds it), its graph edges, state and timings;
//! - one `vars` row per such task: the execution's own scope data.
//!
//! Timings come from the projection itself: the aggregate keeps no wall-clock
//! stamps, so a row's `timestamp` is fixed on first projection and
//! `start_time`/`end_time` are stamped when the execution is first seen
//! started/finished (read-modify-write, so a re-projection of a settled run
//! never moves them). A re-entered node (a newer generation) clears
//! `end_time`, since the surviving instance is running again.
//!
//! Nothing here is authoritative: a projection failure is reported by the
//! caller and never fails the operation, and `removable` — set only once every
//! execution is terminal *and* every outbox record delivered — is what hands
//! the rows (and the run's directory) to the legacy sweeper.
//!
//! # Version protocol (A5)
//!
//! Every row persists the aggregate version it was projected from in
//! `source_revision`, the `"<graph>.<process>.<operations>"` encoding of
//! [`ProcessVersion`] (a pre-A5 row has no field and decodes as `None`, older
//! than every aggregate). Writes are store-level conditional writes
//! ([`Store::write_projection`]): a row applies only when its source revision
//! is strictly newer than the stored row's. The compare and the write commit
//! as one guarded batch under the document lock, so a projection that stalled
//! after reading an older row can never overwrite a newer one — in this
//! process or across instances sharing the store. The in-process
//! [`RowProjector::projected`] map remains only as a fast path; it is no
//! longer the guard.
//!
//! Two explicit repair modes are the only way to write at an *equal*
//! revision: [`RowProjector::project_repair`] (boot catch-up / the durable
//! repair queue) and [`RowProjector::project_proc_row`] (`removable` also
//! depends on channel delivery rows outside the aggregate version). Both read
//! the same aggregate version, so they can only change derived facts
//! (`removable`, timings that are preserved anyway) — never move a row back
//! to an older state.
//!
//! The durable repair queue is the aggregate set itself: a row that lags is
//! detectable from its persisted revision, so a restart re-scans the
//! aggregates (one boot pass, linear in the number of runs, never in ticks)
//! and re-projects only what is behind. Failures are also queued in memory
//! (one entry per pid, bounded by the aggregate set) and retried by the
//! recovery pass. [`ProjectionMetrics`] exposes the backlog/age gauges and
//! the applied/stale/failed/repaired counters.
//!
//! Ordering contract: a commit's external observers (the lifecycle sink and
//! the outbox transport) run only after its rows are current. If a projection
//! fails, the dispatch skips both, the run is queued for repair, and the
//! outbox records stay `Pending` until the retry pass projects the run again.
//!
//! Read contract: the read model is never consulted by Core or Recovery for
//! an authoritative decision (the aggregate is the only truth). The query API
//! (`Runtime::proc` / `proc:get`) is a *strong* read: it re-projects the
//! aggregate when the proc row's `source_revision` is behind, and otherwise
//! serves the rows. A failed strong read serves the stored rows and leaves
//! the repair queued; [`ProjectionMetrics::lag`] reports the gap.
//!
//! The two terminal predicates are deliberately distinct:
//! [`super::facts::run_settled`] (task terminality — the run holds no admission
//! slot) and [`projection_settled`] (tasks + outbox + operations — the rows may
//! be reclaimed).

use super::{facts::node_facts, model_of, root_variable};
use crate::{
    ActError, Result, Vars,
    scheduler::core::{
        commit::aggregate::ProcessSnapshot,
        model::{
            legacy_task_state::LegacyTaskState,
            task_graph::{TaskId, TaskState},
            transition_plan::ProcessVersion,
            view::NodeTree,
        },
    },
    store::{ConditionalWrite, DbCollectionIden, Store, data},
    utils::consts,
};

use std::sync::Arc;

/// The root-scope variable holding the process env a package wrote through
/// `Context::set_env`. Private (`__`-prefixed), so it never leaks into the
/// expression variable space, but the effect seam seeds the headless scope's
/// env from it — `$env.X` answers the same value for every later task of the
/// run, exactly like the legacy process env.
pub(crate) const ENV_MAP_VAR: &str = "__env";

/// The kernel's durable auto/waiting decision variable: engine state, not
/// task data.
const AUTO_VAR: &str = "auto";

/// The kernel's error variables (the naming the failure operations commit).
const ERROR_CODE_VAR: &str = "error.code";
const ERROR_MESSAGE_VAR: &str = "error.message";

/// What one projection pass wrote, by conditional-write outcome.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectionReport {
    pub(crate) applied: usize,
    pub(crate) unchanged: usize,
    pub(crate) stale: usize,
}

impl ProjectionReport {
    fn record(&mut self, outcome: ConditionalWrite) {
        match outcome {
            ConditionalWrite::Applied => self.applied += 1,
            ConditionalWrite::Unchanged => self.unchanged += 1,
            ConditionalWrite::Stale => self.stale += 1,
        }
    }

    fn merge(&mut self, other: Self) {
        self.applied += other.applied;
        self.unchanged += other.unchanged;
        self.stale += other.stale;
    }
}

/// The read model's observable state (A5).
///
/// `lag` and `oldest_lag_age_ms` are gauges: the runs currently queued for
/// repair (a projection failed and the rows may be behind their aggregate).
/// The counters accumulate for the host's lifetime: rows applied, rows that
/// were already identical, writes refused because a newer projection won
/// (`stale`), projection failures, and runs repaired by the catch-up pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProjectionMetrics {
    /// Runs with a pending durable repair (rows behind their aggregate).
    pub lag: usize,
    /// Age of the oldest pending repair, in milliseconds.
    pub oldest_lag_age_ms: Option<i64>,
    /// Rows committed by a projection.
    pub applied: u64,
    /// Rows whose conditional write found the stored row identical.
    pub unchanged: u64,
    /// Writes refused because the stored row was at least as new.
    pub stale: u64,
    /// Projection passes that failed and were queued for repair.
    pub failed: u64,
    /// Runs whose queued repair later projected successfully.
    pub repaired: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProjectionCounters {
    applied: u64,
    unchanged: u64,
    stale: u64,
    failed: u64,
    repaired: u64,
}

/// Materializes the legacy rows of one aggregate.
#[derive(Clone)]
pub(crate) struct RowProjector {
    store: Arc<Store>,
    /// The write-behind queue: a delivery row created this instant must be
    /// seen by the `removable` decision, so the projector barriers that pid's
    /// shard before it decides (see `project`).
    writer: crate::store::writebehind::StoreWriter,
    /// The engine config's workdir root: the run's own directory
    /// (`<root>/<pid>`) is projected into the proc row's env, which is what
    /// the sweeper reads to reclaim it.
    workdir: Option<std::path::PathBuf>,
    /// The newest aggregate version each (run, row) was projected from.
    ///
    /// A projection is *derived* state and its writers are not serialized: the
    /// driver projects the operations it runs while a client thread projects the
    /// action it just committed, and the write-behind queue applies them in
    /// arrival order — which is not the commit order. An older commit's
    /// projection must therefore never write over a newer one's rows, so each
    /// projection claims its run's version first and an older version is
    /// dropped. The sweeper can forget the proc row's claim before repairing
    /// `removable`, which also depends on channel delivery rows outside the
    /// aggregate version. Committed versions are totally ordered (one commit
    /// wins the CAS and the losing writer re-plans from the winner's version);
    /// the map is in-process, and after a restart the first projection of a
    /// run writes (the rows are re-derived anyway).
    projected: Arc<parking_lot::Mutex<std::collections::HashMap<String, ProcessVersion>>>,
    /// Pids whose durable aggregate was reclaimed. A projection that already
    /// holds the old run's snapshot must not recreate its rows after the
    /// sweeper deleted them, so every projection refuses a tombstoned pid; a
    /// new run under the same pid clears the tombstone before it is created.
    /// (The full generation guard belongs to the projection protocol, A5.)
    reclaimed: Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
    /// One async lock per pid, shared by the projection and the cleanup. A
    /// cleanup waits for an in-flight projection of the run before deleting its
    /// aggregate and setting the tombstone; a projection that starts afterwards
    /// sees the tombstone. Without the lock the two writes are unordered and a
    /// projection could recreate rows behind `RemoveProc`.
    process_locks:
        Arc<parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Read-model counters (see [`ProjectionMetrics`]).
    counters: Arc<parking_lot::Mutex<ProjectionCounters>>,
    /// The durable repair queue's in-memory view: one entry per pid whose
    /// rows may be behind its aggregate, with the instant the repair was
    /// first noticed.
    ///
    /// The queue is *derived*, not authoritative: a restart loses the map and
    /// the boot catch-up rebuilds it from the aggregate set (every aggregate
    /// is re-projected; rows already at the aggregate's version are
    /// `Unchanged`). It is bounded by the number of live aggregates — one
    /// entry per pid, never one per failure — and the driver retries a bounded
    /// batch per recovery pass.
    repairs: Arc<parking_lot::Mutex<std::collections::BTreeMap<String, i64>>>,
}

impl RowProjector {
    pub(crate) fn new(
        store: Arc<Store>,
        writer: crate::store::writebehind::StoreWriter,
        workdir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            store,
            writer,
            workdir,
            projected: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            reclaimed: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
            process_locks: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            counters: Arc::new(parking_lot::Mutex::new(ProjectionCounters::default())),
            repairs: Arc::new(parking_lot::Mutex::new(std::collections::BTreeMap::new())),
        }
    }

    /// Note that a run's rows may be behind its aggregate; the driver retries
    /// it from [`Self::pending_repairs`]. Idempotent per pid and bounded by
    /// the aggregate set (see [`Self::repairs`]).
    pub(crate) fn enqueue_repair(&self, process_id: &str) {
        self.repairs
            .lock()
            .entry(process_id.to_string())
            .or_insert_with(crate::utils::time::time_millis);
    }

    /// The run is no longer behind (a projection succeeded, or its aggregate
    /// was reclaimed).
    pub(crate) fn finish_repair(&self, process_id: &str) {
        self.repairs.lock().remove(process_id);
    }

    /// Whether the run is currently queued for repair.
    pub(crate) fn is_repair_pending(&self, process_id: &str) -> bool {
        self.repairs.lock().contains_key(process_id)
    }

    /// One bounded batch of runs to retry, oldest first.
    pub(crate) fn pending_repairs(&self, limit: usize) -> Vec<String> {
        let repairs = self.repairs.lock();
        let mut entries: Vec<(i64, &String)> =
            repairs.iter().map(|(pid, since)| (*since, pid)).collect();
        entries.sort_unstable();
        entries
            .into_iter()
            .take(limit)
            .map(|(_, pid)| pid.clone())
            .collect()
    }

    /// Count a run the driver repaired.
    pub(crate) fn note_repaired(&self) {
        self.counters.lock().repaired += 1;
    }

    /// The read model's counters and repair backlog.
    pub(crate) fn metrics(&self) -> ProjectionMetrics {
        let counters = *self.counters.lock();
        let repairs = self.repairs.lock();
        let now = crate::utils::time::time_millis();
        ProjectionMetrics {
            lag: repairs.len(),
            oldest_lag_age_ms: repairs
                .values()
                .min()
                .map(|since| now.saturating_sub(*since)),
            applied: counters.applied,
            unchanged: counters.unchanged,
            stale: counters.stale,
            failed: counters.failed,
            repaired: counters.repaired,
        }
    }

    /// The aggregate version a row was projected from, or `None` for a row
    /// written before the protocol.
    fn row_version(raw: &str) -> Option<ProcessVersion> {
        ProcessVersion::decode(raw)
    }

    fn record(&self, report: ProjectionReport) {
        let mut counters = self.counters.lock();
        counters.applied += report.applied as u64;
        counters.unchanged += report.unchanged as u64;
        counters.stale += report.stale as u64;
    }

    fn note_failure(&self) {
        self.counters.lock().failed += 1;
    }

    /// The store-level conditional write (A5), with the projector's counters.
    async fn write<T, F>(&self, row: &T, revision: F, allow_equal: bool) -> Result<ConditionalWrite>
    where
        T: DbCollectionIden
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Send
            + Sync
            + Clone
            + std::fmt::Debug
            + PartialEq
            + 'static,
        F: Fn(&T) -> Option<ProcessVersion>,
    {
        match self
            .store
            .write_projection(row, revision, allow_equal)
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                self.note_failure();
                Err(error)
            }
        }
    }

    fn process_lock(&self, process_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.process_locks
            .lock()
            .entry(process_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Serialize one run's projection with its cleanup (see
    /// [`Self::process_locks`]).
    pub(crate) async fn lock_process(&self, process_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.process_lock(process_id).lock_owned().await
    }

    /// Forget every projection claim of one run and tombstone its pid: the
    /// aggregate is gone, so no in-flight projection may write its rows again.
    pub(crate) fn forget_process(&self, process_id: &str) {
        let prefix = format!("{process_id}\u{1f}");
        self.projected
            .lock()
            .retain(|key, _| !key.starts_with(&prefix));
        self.reclaimed.lock().insert(process_id.to_string());
        self.finish_repair(process_id);
    }

    /// Clear a reclaimed pid's tombstone: a new run is starting under it.
    pub(crate) fn resume(&self, process_id: &str) {
        self.reclaimed.lock().remove(process_id);
    }

    fn is_reclaimed(&self, process_id: &str) -> bool {
        self.reclaimed.lock().contains(process_id)
    }

    /// Claim one row's projection version, or refuse it because a newer
    /// commit's projection of that row already ran (see [`Self::projected`]).
    /// Claimed per row — the proc row under its own key — because an operation's
    /// projection may cover a handful of rows while another commit's covers
    /// the process row alone: one claim for both would let the narrower
    /// projection lock the wider one out of a version they share.
    /// Drop a claim whose projection failed (see [`Self::claim`]).
    fn release(&self, snapshot: &ProcessSnapshot, row: &str) {
        let key = format!("{pid}\u{1f}{row}", pid = snapshot.process_id);
        let mut projected = self.projected.lock();
        if projected.get(&key) == Some(&snapshot.version) {
            projected.remove(&key);
        }
    }

    fn claim(&self, snapshot: &ProcessSnapshot, row: &str, allow_equal: bool) -> bool {
        let mut projected = self.projected.lock();
        let key = format!("{pid}\u{1f}{row}", pid = snapshot.process_id);
        match projected.get(&key) {
            // An explicit repair may re-project the same aggregate version
            // (the durable revision guard still refuses anything older).
            Some(newest) if *newest > snapshot.version => false,
            Some(newest) if *newest == snapshot.version && !allow_equal => false,
            _ => {
                projected.insert(key, snapshot.version.clone());
                true
            }
        }
    }

    /// Drop every projection claim of one run so a settled snapshot may be
    /// re-projected at the same version.
    ///
    /// The projection's writes go through the write-behind queue, so a failed
    /// row write is only visible to the *next* projection at a newer version
    /// (a failed row's claim would otherwise outlive it). A folded plan can be
    /// the run's last commit — the propagation that used to re-project the row
    /// is now in the same batch — and a terminal run has no newer version left
    /// to heal it, so the caller clears the run's claims once and re-projects
    /// the same terminal snapshot.
    pub(crate) fn forget_run(&self, process_id: &str) {
        let prefix = format!("{process_id}\u{1f}");
        self.projected
            .lock()
            .retain(|key, _| !key.starts_with(&prefix));
    }

    fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    /// Project the whole process (proc row, task rows, vars rows) from the
    /// committed snapshot. Idempotent: projecting the same snapshot twice
    /// writes the same rows, and a row already at the snapshot's revision is
    /// `Unchanged`.
    pub(crate) async fn project(&self, snapshot: &ProcessSnapshot) -> Result<ProjectionReport> {
        self.project_inner(snapshot, None, false).await
    }

    /// Re-project a whole run as an explicit repair: task/vars rows may be
    /// written at the same aggregate revision (the durable revision guard
    /// still refuses anything older), and the proc row's `removable` flag is
    /// re-derived. The boot catch-up and the repair queue use this.
    pub(crate) async fn project_repair(
        &self,
        snapshot: &ProcessSnapshot,
    ) -> Result<ProjectionReport> {
        self.project_inner(snapshot, None, true).await
    }

    /// Project the proc row plus the named instances' rows; `None` projects
    /// every instance. An operation only changes a handful of executions, so
    /// the driver passes the ones the commit touched instead of rewriting
    /// every row of the run. Since C5 a row is one *instance* (keyed by
    /// `pid + wire`), so the touched set carries the instances' wires.
    pub(crate) async fn project_nodes(
        &self,
        snapshot: &ProcessSnapshot,
        instances: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<ProjectionReport> {
        self.project_inner(snapshot, instances, false).await
    }

    async fn project_inner(
        &self,
        snapshot: &ProcessSnapshot,
        instances: Option<&std::collections::BTreeSet<String>>,
        allow_equal: bool,
    ) -> Result<ProjectionReport> {
        let _guard = self.process_lock(&snapshot.process_id).lock_owned().await;
        if self.is_reclaimed(&snapshot.process_id) {
            return Ok(ProjectionReport::default());
        }
        let Some(model) = model_of(snapshot)? else {
            // An aggregate seeded before the model fact existed: nothing to
            // project the rows from.
            return Ok(ProjectionReport::default());
        };
        let mut tree = NodeTree::new();
        tree.load(&model)?;
        let mut report = ProjectionReport::default();

        // The proc row is derived from the root task's state. Whenever it is
        // projected, project the root task row too: otherwise a commit that
        // touches only variables/outbox can advance the proc row to a terminal
        // state while the root task row still reads `running`.
        let root = snapshot
            .tasks
            .keys()
            .filter(|id| id.node.as_str() == consts::TASK_ROOT_TID)
            .min_by_key(|id| id.generation);
        for task_id in snapshot.tasks.keys() {
            let wire = task_id.wire(&snapshot.process_id);
            let is_root = root.is_some_and(|root| root == task_id);
            if let Some(instances) = instances
                && !instances.contains(&wire)
                && !is_root
            {
                continue;
            }
            if !self.claim(snapshot, &wire, allow_equal) {
                continue;
            }
            match self
                .project_task(snapshot, &tree, task_id, &wire, allow_equal)
                .await
            {
                Ok(rows) => report.merge(rows),
                Err(error) => {
                    // The claim may not outlive a write that never landed —
                    // the next projection of this version (a retry, a later
                    // flush) must be allowed to write the row.
                    self.release(snapshot, &wire);
                    self.record(report);
                    return Err(error);
                }
            }
        }
        // The proc row is the summary row: write it last so a reader that sees
        // the terminal proc state can also see the task rows it summarizes.
        if self.claim(snapshot, "proc", allow_equal) {
            match self.project_proc(snapshot, &model, allow_equal).await {
                Ok(rows) => report.merge(rows),
                Err(error) => {
                    self.release(snapshot, "proc");
                    self.record(report);
                    return Err(error);
                }
            }
        }
        self.record(report);
        Ok(report)
    }

    /// Project only the process row: what the outbox's settling pass needs
    /// (the `removable` flag), with nothing else changed. The row may be
    /// rewritten at the same aggregate revision because `removable` also
    /// depends on channel delivery rows outside that version.
    pub(crate) async fn project_proc_row(
        &self,
        snapshot: &ProcessSnapshot,
    ) -> Result<ProjectionReport> {
        let _guard = self.process_lock(&snapshot.process_id).lock_owned().await;
        if self.is_reclaimed(&snapshot.process_id) {
            return Ok(ProjectionReport::default());
        }
        let Some(model) = model_of(snapshot)? else {
            return Ok(ProjectionReport::default());
        };
        let mut report = ProjectionReport::default();
        if self.claim(snapshot, "proc", true) {
            match self.project_proc(snapshot, &model, true).await {
                Ok(rows) => report.merge(rows),
                Err(error) => {
                    self.release(snapshot, "proc");
                    self.record(report);
                    return Err(error);
                }
            }
        }
        self.record(report);
        Ok(report)
    }

    async fn project_proc(
        &self,
        snapshot: &ProcessSnapshot,
        model: &crate::model::Workflow,
        allow_equal: bool,
    ) -> Result<ProjectionReport> {
        let Some(root) = snapshot
            .tasks
            .keys()
            .filter(|id| id.node.as_str() == consts::TASK_ROOT_TID)
            .min_by_key(|id| id.generation)
            .cloned()
        else {
            return Ok(ProjectionReport::default());
        };
        let state = snapshot
            .tasks
            .get(&root)
            .map(|task| task.state)
            .unwrap_or(TaskState::Created);

        let collection = self.store().procs();
        let existing = collection.find_opt(&snapshot.process_id).await?;
        let now = crate::utils::time::time_millis();
        let row = data::Proc {
            source_revision: snapshot.version.encode(),
            id: snapshot.process_id.clone(),
            state: proc_state(state).into(),
            mid: model.id.clone(),
            name: model.name.clone(),
            start_time: if state == TaskState::Created {
                0
            } else {
                stamp(existing.as_ref().map(|row| row.start_time), now)
            },
            end_time: if state.is_terminal() {
                stamp(existing.as_ref().map(|row| row.end_time), now)
            } else {
                0
            },
            timestamp: stamp(existing.as_ref().map(|row| row.timestamp), now),
            model: model.to_json()?,
            env: self.proc_env(snapshot, &root)?,
            err: error_of(snapshot, &root),
            removable: self.removable(snapshot).await?,
            v: data::Proc::version(),
        };
        let mut report = ProjectionReport::default();
        let outcome = self
            .write(
                &row,
                |row: &data::Proc| Self::row_version(&row.source_revision),
                allow_equal,
            )
            .await?;
        report.record(outcome);
        Ok(report)
    }

    /// Whether the sweeper may delete the run: every execution settled, every
    /// outbox record delivered, and no channel delivery of its messages still
    /// open — the legacy `try_mark_removable` rule. An un-acked delivery keeps
    /// the rows (and the run's directory) alive for the retry timer, so the
    /// kernel must not sweep it away. The process's own write shard is
    /// barriered first: a delivery row the transport queued this instant must
    /// be seen, or the sweeper would delete behind it.
    async fn removable(&self, snapshot: &ProcessSnapshot) -> Result<bool> {
        if !projection_settled(snapshot) {
            return Ok(false);
        }
        self.writer.flush_pid(&snapshot.process_id).await?;
        Ok(!self
            .store()
            .has_unsettled_deliveries(&snapshot.process_id)
            .await?)
    }

    /// The process env, as the legacy proc row carries it: the caller
    /// authority, the run's directory, and whatever a package wrote through
    /// `Context::set_env` (the durable `__env` map).
    fn proc_env(&self, snapshot: &ProcessSnapshot, root: &TaskId) -> Result<String> {
        let mut env = Vars::new();
        if let Some(owner) = snapshot
            .variables
            .get(&(root.clone(), consts::PROC_OWNER.to_string()))
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        {
            env.insert(consts::PROC_OWNER.to_string(), owner);
        }
        if let Some(root_dir) = &self.workdir {
            env.set(
                consts::PROC_WORKDIR,
                root_dir.join(&snapshot.process_id).display().to_string(),
            );
        }
        for (name, value) in env_map(snapshot)? {
            env.insert(name, value);
        }
        Ok(env.to_string())
    }

    async fn project_task(
        &self,
        snapshot: &ProcessSnapshot,
        tree: &NodeTree,
        task_id: &TaskId,
        wire: &str,
        allow_equal: bool,
    ) -> Result<ProjectionReport> {
        let task = snapshot
            .tasks
            .get(task_id)
            .expect("projected execution must exist");
        // The row is one *instance* (C5): its `tid` is the instance's wire,
        // while `node_data.id` (`nid`) stays the model node — the split the
        // legacy row already had (`tid = "$"`, `node_data.id = <model id>`
        // for the root).
        let node_id = task_id.node.as_str();
        let node = if node_id == consts::TASK_ROOT_TID {
            tree.root()
        } else {
            tree.node(node_id)
        };
        let Some(node) = node else {
            // An execution whose model node is gone can only come from a
            // corrupt aggregate; the rows are re-derived on the next commit,
            // so the projection reports and skips it rather than failing the
            // operation.
            tracing::warn!(
                pid = %snapshot.process_id,
                node = node_id,
                "row projection: model has no node; task row skipped"
            );
            return Ok(ProjectionReport::default());
        };

        // The row and the node's message share one derivation (C8b): the row
        // takes the declaration's kind — a `uses` step stays "step" here even
        // though the message vocabulary names it an act — and the name both
        // carry (the root's is the workflow's `model.name`).
        let facts = node_facts(&tree.model, node_id);
        let row_id = crate::utils::Id::new(&snapshot.process_id, wire).id();
        let collection = self.store().tasks();
        let existing = collection.find_opt(&row_id).await?;
        let now = crate::utils::time::time_millis();
        let row = data::Task {
            source_revision: snapshot.version.encode(),
            id: row_id.clone(),
            pid: snapshot.process_id.clone(),
            tid: wire.to_string(),
            node_data: node.to_string()?,
            // The row's kind is the declaration's node kind (`NodeFacts::declared_kind`),
            // never the message level: a `uses` step stays "step" here while
            // its message is an act. This word is a pinned read-model contract,
            // not a spelling of the message vocabulary (C6, §10).
            kind: facts.declared_kind.to_string(),
            // The model chain (D3): predecessor/successor/parent stay node
            // ids — they say which *node* follows, not which instance.
            prev: task
                .predecessors
                .iter()
                .max_by_key(|id| id.generation)
                .map(|id| id.node.as_str().to_owned()),
            next: successors(snapshot, task_id),
            parent: task.parent.as_ref().map(|id| id.node.as_str().to_owned()),
            name: facts.name,
            state: legacy_task_state(task.state).into(),
            err: error_of(snapshot, task_id),
            start_time: if task.state == TaskState::Created {
                0
            } else {
                stamp(existing.as_ref().map(|row| row.start_time), now)
            },
            end_time: if task.state.is_terminal() {
                stamp(existing.as_ref().map(|row| row.end_time), now)
            } else {
                0
            },
            timestamp: stamp(existing.as_ref().map(|row| row.timestamp), now),
            v: data::Task::version(),
        };
        let mut report = ProjectionReport::default();
        let task_outcome = self
            .write(
                &row,
                |row: &data::Task| Self::row_version(&row.source_revision),
                allow_equal,
            )
            .await?;
        report.record(task_outcome);
        // `Stale` means a newer projection already wrote this row. Its scope
        // row is at least as new too, so leave both alone: only the winning
        // projection may settle the task's deliveries.
        if task_outcome == ConditionalWrite::Stale {
            return Ok(report);
        }

        // A task that just turned terminal settles its channel deliveries —
        // the legacy writer's close. An un-acked delivery of a finished task
        // can never be acked into anything else, and a row left open would
        // keep the whole run (and its directory) out of the sweeper's reach.
        // Deliveries key on the message `tid` — the wire since C5.
        let was_terminal = existing
            .as_ref()
            .map(|row| LegacyTaskState::from(row.state.as_str()).is_completed())
            .unwrap_or(false);
        if task.state.is_terminal() && !was_terminal {
            self.store()
                .close_deliveries(&snapshot.process_id, wire)
                .await?;
        }

        let data = scope_data(snapshot, task_id);
        if data.is_empty() {
            return Ok(report);
        }
        let row = data::TaskVars {
            source_revision: snapshot.version.encode(),
            id: row_id.clone(),
            pid: snapshot.process_id.clone(),
            tid: wire.to_string(),
            data: serde_json::Value::Object(data).to_string(),
            // The aggregate carries no sealed scope: a snapshot seal is a
            // per-effect resolution, not durable run state (see `effects`).
            sealed: String::new(),
            v: data::TaskVars::version(),
        };
        let vars_outcome = self
            .write(
                &row,
                |row: &data::TaskVars| Self::row_version(&row.source_revision),
                allow_equal,
            )
            .await?;
        report.record(vars_outcome);
        Ok(report)
    }
}

/// The process env a package wrote through `Context::set_env`, read from the
/// durable root-scope map.
pub(crate) fn env_map(snapshot: &ProcessSnapshot) -> Result<Vec<(String, serde_json::Value)>> {
    let Some(raw) = root_variable(snapshot, ENV_MAP_VAR)? else {
        return Ok(Vec::new());
    };
    let vars: Vars = serde_json::from_str(&raw)
        .map_err(|err| ActError::Runtime(format!("aggregate env map is invalid: {err}")))?;
    Ok(vars
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect())
}

/// Whether the **read model** may be handed to the sweeper: every execution
/// terminal, every outbox record delivered and every operation retired.
///
/// This is deliberately stronger than [`super::facts::run_settled`], which
/// only looks at task terminality and decides whether the run still holds an
/// admission slot. The two are different questions:
///
/// - `run_settled` — the run has no more work to drive; its slot is free.
/// - `projection_settled` (this function) — the rows may be deleted, because
///   no message can still be produced and no operation still needs an owner.
///
/// A record the transport could not classify (`DeliveryUnknown`,
/// `Quarantined`) or an operation that still needs one (`EffectUnknown`,
/// `Quarantined`, schedulable work) keeps the rows alive for reconciliation,
/// exactly like an open legacy delivery row.
pub(crate) fn projection_settled(snapshot: &ProcessSnapshot) -> bool {
    snapshot.is_reclaimable()
}

/// The model chain following one execution (D3: node ids, not instances):
/// every instance that names it a predecessor contributes its own node, so a
/// node with several live generations still names its successor once.
/// Deterministic (sorted, deduplicated).
fn successors(snapshot: &ProcessSnapshot, task_id: &TaskId) -> Vec<String> {
    let mut next: Vec<String> = snapshot
        .tasks
        .iter()
        .filter(|(_, task)| task.predecessors.contains(task_id))
        .map(|(id, _)| id.node.as_str().to_owned())
        .collect();
    next.sort();
    next.dedup();
    next
}

/// The scope data of one execution: its own variables, decoded, with the
/// engine's private facts and the `auto` marker left out — the task data a
/// legacy row holds.
fn scope_data(
    snapshot: &ProcessSnapshot,
    task: &TaskId,
) -> serde_json::Map<String, serde_json::Value> {
    let mut data = serde_json::Map::new();
    for ((scope, name), raw) in &snapshot.variables {
        if scope != task || consts::is_private_key(name) || name == AUTO_VAR {
            continue;
        }
        data.insert(
            name.clone(),
            serde_json::from_str(raw).unwrap_or(serde_json::Value::String(raw.clone())),
        );
    }
    data
}

/// The failed execution's error, in the legacy `Error` shape
/// (`{"ecode":...,"message":...}`).
fn error_of(snapshot: &ProcessSnapshot, task: &TaskId) -> Option<String> {
    let task_state = snapshot.tasks.get(task)?.state;
    if task_state != TaskState::Failed {
        return None;
    }
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(task.clone(), name.to_string()))
            .map(|raw| serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.clone()))
            .unwrap_or_default()
    };
    let err = crate::Error::new(&read(ERROR_MESSAGE_VAR), &read(ERROR_CODE_VAR));
    serde_json::to_string(&err).ok()
}

/// One execution state as the legacy task state the read model speaks.
pub(crate) fn legacy_task_state(state: TaskState) -> LegacyTaskState {
    match state {
        // A pre-created chain node is the legacy `Pending` task: created, not
        // yet woken by its predecessor.
        TaskState::Created => LegacyTaskState::Pending,
        TaskState::Ready => LegacyTaskState::Ready,
        TaskState::Running => LegacyTaskState::Running,
        TaskState::Waiting => LegacyTaskState::Interrupt,
        TaskState::Completed => LegacyTaskState::Completed,
        TaskState::Submitted => LegacyTaskState::Submitted,
        TaskState::Backed => LegacyTaskState::Backed,
        TaskState::Cancelled => LegacyTaskState::Cancelled,
        TaskState::Failed => LegacyTaskState::Error,
        TaskState::Aborted => LegacyTaskState::Aborted,
        TaskState::Skipped => LegacyTaskState::Skipped,
        TaskState::Removed => LegacyTaskState::Removed,
    }
}

/// The process state of a run whose root is in `state`. The legacy process is
/// `None` until its root task is created, `Running` while the root lives, and
/// the root's own terminal state afterwards.
pub(crate) fn proc_state(state: TaskState) -> LegacyTaskState {
    match state {
        TaskState::Created => LegacyTaskState::None,
        TaskState::Ready | TaskState::Running | TaskState::Waiting => LegacyTaskState::Running,
        other => legacy_task_state(other),
    }
}

/// Keep a durable stamp once it exists, otherwise take `now`.
fn stamp(existing: Option<i64>, now: i64) -> i64 {
    match existing {
        Some(value) if value > 0 => value,
        _ => now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_states_map_onto_the_legacy_task_vocabulary() {
        // The parity folding the migration notes pin: Waiting is the legacy
        // Interrupt, Failed is Error.
        assert_eq!(
            legacy_task_state(TaskState::Waiting),
            LegacyTaskState::Interrupt
        );
        assert_eq!(legacy_task_state(TaskState::Failed), LegacyTaskState::Error);
        assert_eq!(
            legacy_task_state(TaskState::Created),
            LegacyTaskState::Pending
        );
        assert_eq!(
            legacy_task_state(TaskState::Completed),
            LegacyTaskState::Completed
        );
        assert_eq!(
            legacy_task_state(TaskState::Aborted),
            LegacyTaskState::Aborted
        );
    }

    #[test]
    fn proc_state_tracks_the_root_and_predates_creation() {
        assert_eq!(proc_state(TaskState::Created), LegacyTaskState::None);
        assert_eq!(proc_state(TaskState::Running), LegacyTaskState::Running);
        assert_eq!(proc_state(TaskState::Waiting), LegacyTaskState::Running);
        assert_eq!(proc_state(TaskState::Completed), LegacyTaskState::Completed);
        assert_eq!(proc_state(TaskState::Failed), LegacyTaskState::Error);
        assert_eq!(proc_state(TaskState::Skipped), LegacyTaskState::Skipped);
    }
}
