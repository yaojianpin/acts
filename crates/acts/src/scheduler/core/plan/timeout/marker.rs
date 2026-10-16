use super::*;

/// The cost start of one execution, written by its own start hop (`Ready →
/// Running`/`Waiting`). Not part of the expression variable space: it is a
/// kernel fact, so it stays `__`-private and never reaches a headless scope.
///
/// The legacy `Task` stamped its *creation* time and measured `cost()` from it.
/// The kernel pre-creates a run's chain executions at seed time, so creation
/// time would measure the whole run's age; the start hop is the closest durable
/// analogue — it is the moment the execution began its own work, which is what
/// the legacy stamp meant for a step its predecessor scheduled.
pub(crate) const STARTED_VAR: &str = "__started";

/// One declared timeout branch's durable admission facts: the model-supplied
/// parts (the guard itself is evaluated by the host, on the engine's own
/// environment).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimeoutBranch {
    pub(crate) node: String,
    pub(crate) model_node: NodeId,
    /// The branch's declared model vars, seeded on its own scope.
    pub(crate) vars: Vec<(String, String)>,
    /// The import's auto decision for the branch (whether it self-completes).
    pub(crate) auto: bool,
}

/// Which declared branch the next dispatch round examines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimeoutCandidate {
    /// The i-th declared branch has not fired and has no live execution: this
    /// round evaluates its guard and dispatches it when the guard holds.
    Branch(usize),
    /// The first un-fired branch is in flight: the round stops here — exactly
    /// like the legacy dispatcher — so the branch declared after it waits for
    /// its turn instead of running ahead of it.
    Blocked,
    /// Every declared branch already fired.
    None,
}

/// The branches that already fired for one execution, from the one-shot marker
/// (the legacy `__timeouts` task datum, a JSON array).
pub(crate) fn claimed(snapshot: &ProcessSnapshot, owner: &TaskId) -> Vec<String> {
    marker(snapshot, owner).unwrap_or_default()
}

fn marker(snapshot: &ProcessSnapshot, owner: &TaskId) -> Option<Vec<String>> {
    let raw = snapshot
        .variables
        .get(&(owner.clone(), consts::TASK_TIMEOUTS.to_string()))?;
    serde_json::from_str(raw).ok()
}

/// The marker after `branch` fired: the one-shot claim of one branch,
/// appended to the ones already taken (legacy `Task::claim_timeout`).
pub(crate) fn claim(claimed: &[String], branch: &str) -> Vec<String> {
    let mut next = claimed.to_vec();
    if !next.iter().any(|id| id == branch) {
        next.push(branch.to_string());
    }
    next
}

/// The cost of one execution in milliseconds: what has passed since its start
/// hop, or zero when that stamp is missing (an aggregate seeded before the
/// timeout slice) or in the future (a clock that stepped backwards).
pub(crate) fn cost_ms(now_ms: i64, started: Option<&str>) -> i64 {
    let started = started
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(now_ms);
    now_ms.saturating_sub(started).max(0)
}

/// Whether one declared branch has a live (non-terminal) execution under
/// `owner` — the "in flight" half of the legacy dispatcher's check.
pub(crate) fn has_live_child(snapshot: &ProcessSnapshot, owner: &TaskId, branch: &str) -> bool {
    snapshot.tasks.iter().any(|(id, task)| {
        is_child_of(task.parent.as_ref(), owner, id, branch) && !task.state.is_terminal()
    })
}

fn is_child_of(parent: Option<&TaskId>, owner: &TaskId, id: &TaskId, branch: &str) -> bool {
    parent == Some(owner) && id.node == branch
}

/// Whether any of the declared timeout branches still has a live execution:
/// the owner may not complete before the branches it fired settle (the legacy
/// `auto_complete` counts every child task, timeout ones included).
pub(crate) fn has_live_children(
    snapshot: &ProcessSnapshot,
    owner: &TaskId,
    branches: &[TimeoutBranch],
) -> bool {
    branches
        .iter()
        .any(|branch| has_live_child(snapshot, owner, &branch.node))
}

/// Which branch the next round examines: declaration order, skipping the ones
/// that already fired, stopping at the first branch that is in flight.
pub(crate) fn candidate(
    snapshot: &ProcessSnapshot,
    owner: &TaskId,
    branches: &[TimeoutBranch],
) -> TimeoutCandidate {
    let fired = claimed(snapshot, owner);
    for (index, branch) in branches.iter().enumerate() {
        if fired.iter().any(|id| id == &branch.node) {
            continue;
        }
        if has_live_child(snapshot, owner, &branch.node) {
            return TimeoutCandidate::Blocked;
        }
        return TimeoutCandidate::Branch(index);
    }
    TimeoutCandidate::None
}
