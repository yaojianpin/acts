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
//! caller and never fails the hop (the rows are re-derived from the aggregate
//! on the next commit), and `removable` — set only once every execution is
//! terminal *and* every outbox record delivered — is what hands the rows (and
//! the run's directory) to the legacy sweeper.

use super::{model_of, root_variable};
use crate::{
    ActError, Result, Vars,
    scheduler::core::{
        commit::process::ProcessSnapshot,
        model::{
            execution_graph::{ExecutionId, ExecutionState},
            task_state::TaskState,
            transition_plan::ProcessVersion,
            view::NodeTree,
        },
    },
    store::{DbCollection, DbCollectionIden, Store, data},
    utils::consts,
};

use std::collections::BTreeMap;
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

/// The kernel's error variables (the naming the failure hops commit).
const ERROR_CODE_VAR: &str = "error.code";
const ERROR_MESSAGE_VAR: &str = "error.message";

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
    /// driver projects the hops it runs while a client thread projects the
    /// action it just committed, and the write-behind queue applies them in
    /// arrival order — which is not the commit order. An older commit's
    /// projection must therefore never write over a newer one's rows, so each
    /// projection claims its run's version first and a version that is not
    /// newer is dropped. Committed versions are totally ordered (one commit
    /// wins the CAS and the losing writer re-plans from the winner's version),
    /// which makes the check sound; the map is in-process, and after a restart
    /// the first projection of a run writes (the rows are re-derived anyway).
    projected: Arc<parking_lot::Mutex<std::collections::HashMap<String, ProcessVersion>>>,
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
        }
    }

    /// Claim one row's projection version, or refuse it because a newer
    /// commit's projection of that row already ran (see [`Self::projected`]).
    /// Claimed per row — the proc row under its own key — because a hop's
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

    fn claim(&self, snapshot: &ProcessSnapshot, row: &str) -> bool {
        let mut projected = self.projected.lock();
        let key = format!("{pid}\u{1f}{row}", pid = snapshot.process_id);
        match projected.get(&key) {
            Some(newest) if *newest >= snapshot.version => false,
            _ => {
                projected.insert(key, snapshot.version.clone());
                true
            }
        }
    }

    fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    /// Project the whole process (proc row, task rows, vars rows) from the
    /// committed snapshot. Idempotent: projecting the same snapshot twice
    /// writes the same rows.
    pub(crate) async fn project(&self, snapshot: &ProcessSnapshot) -> Result<()> {
        self.project_nodes(snapshot, None).await
    }

    /// Project the proc row plus the named nodes' rows; `None` projects every
    /// node. A hop only changes a handful of executions, so the driver passes
    /// the ones the commit touched instead of rewriting every row of the run.
    pub(crate) async fn project_nodes(
        &self,
        snapshot: &ProcessSnapshot,
        nodes: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<()> {
        let Some(model) = model_of(snapshot)? else {
            // An aggregate seeded before the model fact existed: nothing to
            // project the rows from.
            return Ok(());
        };
        let mut tree = NodeTree::new();
        tree.load(&model)?;

        // The newest generation of every node is the row's task instance: the
        // store keys a task by `pid + tid` and the kernel's tid is the model
        // node id, so one surviving instance may be projected per node.
        let mut latest: BTreeMap<String, ExecutionId> = BTreeMap::new();
        for id in snapshot.executions.keys() {
            match latest.get(&id.task_id) {
                Some(current) if current.generation >= id.generation => {}
                _ => {
                    latest.insert(id.task_id.clone(), id.clone());
                }
            }
        }

        if self.claim(snapshot, "proc")
            && let Err(error) = self.project_proc(snapshot, &model).await
        {
            self.release(snapshot, "proc");
            return Err(error);
        }
        for (task_id, execution_id) in &latest {
            if let Some(nodes) = nodes
                && !nodes.contains(task_id)
            {
                continue;
            }
            if !self.claim(snapshot, task_id) {
                continue;
            }
            if let Err(error) = self
                .project_task(snapshot, &tree, &latest, task_id, execution_id)
                .await
            {
                // The claim may not outlive a write that never landed — the
                // next projection of this version (a retry, a later flush)
                // must be allowed to write the row.
                self.release(snapshot, task_id);
                return Err(error);
            }
        }
        Ok(())
    }

    /// Project only the process row: what the outbox's settling pass needs
    /// (the `removable` flag), with nothing else changed.
    pub(crate) async fn project_proc_row(&self, snapshot: &ProcessSnapshot) -> Result<()> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(());
        };
        if self.claim(snapshot, "proc")
            && let Err(error) = self.project_proc(snapshot, &model).await
        {
            self.release(snapshot, "proc");
            return Err(error);
        }
        Ok(())
    }

    async fn project_proc(
        &self,
        snapshot: &ProcessSnapshot,
        model: &crate::model::Workflow,
    ) -> Result<()> {
        let Some(root) = snapshot
            .executions
            .keys()
            .filter(|id| id.task_id == consts::TASK_ROOT_TID)
            .min_by_key(|id| id.generation)
            .cloned()
        else {
            return Ok(());
        };
        let state = snapshot
            .executions
            .get(&root)
            .map(|execution| execution.state)
            .unwrap_or(ExecutionState::Created);

        let collection = self.store().procs();
        let existing = collection.find_opt(&snapshot.process_id).await?;
        let now = crate::utils::time::time_millis();
        let row = data::Proc {
            id: snapshot.process_id.clone(),
            state: proc_state(state).into(),
            mid: model.id.clone(),
            name: model.name.clone(),
            start_time: if state == ExecutionState::Created {
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
        write(&collection, existing.as_ref(), &row).await
    }

    /// Whether the sweeper may delete the run: every execution settled, every
    /// outbox record delivered, and no channel delivery of its messages still
    /// open — the legacy `try_mark_removable` rule. An un-acked delivery keeps
    /// the rows (and the run's directory) alive for the retry timer, so the
    /// kernel must not sweep it away. The process's own write shard is
    /// barriered first: a delivery row the transport queued this instant must
    /// be seen, or the sweeper would delete behind it.
    async fn removable(&self, snapshot: &ProcessSnapshot) -> Result<bool> {
        if !settled(snapshot) {
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
    fn proc_env(&self, snapshot: &ProcessSnapshot, root: &ExecutionId) -> Result<String> {
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
        latest: &BTreeMap<String, ExecutionId>,
        task_id: &str,
        execution_id: &ExecutionId,
    ) -> Result<()> {
        let execution = snapshot
            .executions
            .get(execution_id)
            .expect("projected execution must exist");
        // The root task's tid is `$` while its node is the workflow node the
        // tree is rooted at (the legacy root row has the same split: `tid =
        // "$"`, `node_data.id = <model id>`).
        let node = if task_id == consts::TASK_ROOT_TID {
            tree.root()
        } else {
            tree.node(task_id)
        };
        let Some(node) = node else {
            // An execution whose model node is gone can only come from a
            // corrupt aggregate; the rows are re-derived on the next commit,
            // so the projection reports and skips it rather than failing the
            // hop.
            tracing::warn!(
                pid = %snapshot.process_id,
                node = task_id,
                "row projection: model has no node; task row skipped"
            );
            return Ok(());
        };

        let row_id = crate::utils::Id::new(&snapshot.process_id, task_id).id();
        let collection = self.store().tasks();
        let existing = collection.find_opt(&row_id).await?;
        let now = crate::utils::time::time_millis();
        let row = data::Task {
            id: row_id.clone(),
            pid: snapshot.process_id.clone(),
            tid: task_id.to_string(),
            node_data: node.to_string()?,
            kind: node.kind().to_string(),
            prev: execution
                .predecessors
                .iter()
                .max_by_key(|id| id.generation)
                .map(|id| id.task_id.clone()),
            next: successors(snapshot, latest, execution_id),
            parent: execution.parent.as_ref().map(|id| id.task_id.clone()),
            name: node.name(),
            state: task_state(execution.state).into(),
            err: error_of(snapshot, execution_id),
            start_time: if execution.state == ExecutionState::Created {
                0
            } else {
                stamp(existing.as_ref().map(|row| row.start_time), now)
            },
            end_time: if execution.state.is_terminal() {
                stamp(existing.as_ref().map(|row| row.end_time), now)
            } else {
                0
            },
            timestamp: stamp(existing.as_ref().map(|row| row.timestamp), now),
            v: data::Task::version(),
        };
        write(&collection, existing.as_ref(), &row).await?;

        // A task that just turned terminal settles its channel deliveries —
        // the legacy writer's close. An un-acked delivery of a finished task
        // can never be acked into anything else, and a row left open would
        // keep the whole run (and its directory) out of the sweeper's reach.
        let was_terminal = existing
            .as_ref()
            .map(|row| TaskState::from(row.state.as_str()).is_completed())
            .unwrap_or(false);
        if execution.state.is_terminal() && !was_terminal {
            self.store()
                .close_deliveries(&snapshot.process_id, task_id)
                .await?;
        }

        let data = scope_data(snapshot, execution_id);
        if data.is_empty() {
            return Ok(());
        }
        let collection = self.store().vars();
        let existing = collection.find_opt(&row_id).await?;
        let row = data::TaskVars {
            id: row_id.clone(),
            pid: snapshot.process_id.clone(),
            tid: task_id.to_string(),
            data: serde_json::Value::Object(data).to_string(),
            // The aggregate carries no sealed scope: a snapshot seal is a
            // per-effect resolution, not durable run state (see `effects`).
            sealed: String::new(),
            v: data::TaskVars::version(),
        };
        write(&collection, existing.as_ref(), &row).await
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

/// Whether the sweeper may delete the run: every execution settled and every
/// outbox record delivered. A record the transport could not classify
/// (`DeliveryUnknown`, `Quarantined`) keeps the rows alive for reconciliation,
/// exactly like an open legacy delivery row.
pub(crate) fn settled(snapshot: &ProcessSnapshot) -> bool {
    !snapshot.executions.is_empty()
        && snapshot
            .executions
            .values()
            .all(|execution| execution.state.is_terminal())
        && snapshot.outbox.values().all(|record| {
            matches!(
                record.state,
                crate::scheduler::core::outbox::records::OutboxState::Delivered { .. }
            )
        })
}

/// The successor task ids of one execution: every node (newest generation)
/// that names it a predecessor. Deterministic (sorted).
fn successors(
    snapshot: &ProcessSnapshot,
    latest: &BTreeMap<String, ExecutionId>,
    execution_id: &ExecutionId,
) -> Vec<String> {
    let mut next: Vec<String> = latest
        .values()
        .filter(|id| {
            snapshot
                .executions
                .get(*id)
                .map(|execution| execution.predecessors.contains(execution_id))
                .unwrap_or(false)
        })
        .map(|id| id.task_id.clone())
        .collect();
    next.sort();
    next
}

/// The scope data of one execution: its own variables, decoded, with the
/// engine's private facts and the `auto` marker left out — the task data a
/// legacy row holds.
fn scope_data(
    snapshot: &ProcessSnapshot,
    execution: &ExecutionId,
) -> serde_json::Map<String, serde_json::Value> {
    let mut data = serde_json::Map::new();
    for ((scope, name), raw) in &snapshot.variables {
        if scope != execution || consts::is_private_key(name) || name == AUTO_VAR {
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
fn error_of(snapshot: &ProcessSnapshot, execution: &ExecutionId) -> Option<String> {
    let execution_state = snapshot.executions.get(execution)?.state;
    if execution_state != ExecutionState::Failed {
        return None;
    }
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(execution.clone(), name.to_string()))
            .map(|raw| serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.clone()))
            .unwrap_or_default()
    };
    let err = crate::Error::new(&read(ERROR_MESSAGE_VAR), &read(ERROR_CODE_VAR));
    serde_json::to_string(&err).ok()
}

/// One execution state as the legacy task state the read model speaks.
pub(crate) fn task_state(state: ExecutionState) -> TaskState {
    match state {
        // A pre-created chain node is the legacy `Pending` task: created, not
        // yet woken by its predecessor.
        ExecutionState::Created => TaskState::Pending,
        ExecutionState::Ready => TaskState::Ready,
        ExecutionState::Running => TaskState::Running,
        ExecutionState::Waiting => TaskState::Interrupt,
        ExecutionState::Completed => TaskState::Completed,
        ExecutionState::Submitted => TaskState::Submitted,
        ExecutionState::Backed => TaskState::Backed,
        ExecutionState::Cancelled => TaskState::Cancelled,
        ExecutionState::Failed => TaskState::Error,
        ExecutionState::Aborted => TaskState::Aborted,
        ExecutionState::Skipped => TaskState::Skipped,
        ExecutionState::Removed => TaskState::Removed,
    }
}

/// The process state of a run whose root is in `state`. The legacy process is
/// `None` until its root task is created, `Running` while the root lives, and
/// the root's own terminal state afterwards.
pub(crate) fn proc_state(state: ExecutionState) -> TaskState {
    match state {
        ExecutionState::Created => TaskState::None,
        ExecutionState::Ready | ExecutionState::Running | ExecutionState::Waiting => {
            TaskState::Running
        }
        other => task_state(other),
    }
}

/// Keep a durable stamp once it exists, otherwise take `now`.
fn stamp(existing: Option<i64>, now: i64) -> i64 {
    match existing {
        Some(value) if value > 0 => value,
        _ => now,
    }
}

/// Upsert one projected row: replace an existing document, create a missing
/// one — and skip the write when the stored document is already identical.
///
/// A hop re-projects the rows it touched even when their content did not move
/// (the root's proc row changes only when the run starts and when it settles,
/// while a task row repeats a state it already carries), and every write also
/// writes the row's index keys. The projected row is compared against the one
/// just read, so an unchanged hop costs reads instead of writes.
async fn write<T>(
    collection: &Arc<dyn DbCollection<Item = T>>,
    existing: Option<&T>,
    row: &T,
) -> Result<()>
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
{
    match existing {
        Some(current) if current == row => {}
        Some(_) => {
            collection.update(row).await?;
        }
        None => {
            collection.create(row).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_states_map_onto_the_legacy_task_vocabulary() {
        // The parity folding the migration notes pin: Waiting is the legacy
        // Interrupt, Failed is Error.
        assert_eq!(task_state(ExecutionState::Waiting), TaskState::Interrupt);
        assert_eq!(task_state(ExecutionState::Failed), TaskState::Error);
        assert_eq!(task_state(ExecutionState::Created), TaskState::Pending);
        assert_eq!(task_state(ExecutionState::Completed), TaskState::Completed);
        assert_eq!(task_state(ExecutionState::Aborted), TaskState::Aborted);
    }

    #[test]
    fn proc_state_tracks_the_root_and_predates_creation() {
        assert_eq!(proc_state(ExecutionState::Created), TaskState::None);
        assert_eq!(proc_state(ExecutionState::Running), TaskState::Running);
        assert_eq!(proc_state(ExecutionState::Waiting), TaskState::Running);
        assert_eq!(proc_state(ExecutionState::Completed), TaskState::Completed);
        assert_eq!(proc_state(ExecutionState::Failed), TaskState::Error);
        assert_eq!(proc_state(ExecutionState::Skipped), TaskState::Skipped);
    }
}
