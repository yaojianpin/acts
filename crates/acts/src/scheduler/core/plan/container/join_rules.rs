//! The join decision rules: what a children census means to one rule, and
//! what a deciding package's answer means.
//!
//! The rules are two families: the counted ones (`all`/`any`/`partial`)
//! decide from the children's census alone ([`decide`]), and the delegated
//! one (`action`) decides by the deciding package's verdict ([`verdict`]).
//! Both are pure policy over the core vocabulary — the shell's join
//! capability (`shell::caps::join`) re-exports them beside its orchestration.

use crate::scheduler::core::model::task_graph::{TaskId, TaskState};

/// One counted child's vote: the state a join rule reads.
///
/// `Skipped` and `Removed` are *withdrawn* — a branch its own guard skipped,
/// or an act the client removed, was never in the vote, so neither the
/// completions nor the threshold count it (and `all` keeps completing over
/// one, exactly as the join always did). `Completed`/`Submitted` are
/// completions; `Failed`/`Cancelled`/`Backed` are settled non-completions
/// that can never pass; anything else is still in flight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Vote {
    Completion,
    Settled,
    Withdrawn,
    Aborted,
    InFlight,
}

pub(crate) fn vote(state: TaskState) -> Vote {
    match state {
        TaskState::Completed | TaskState::Submitted => Vote::Completion,
        TaskState::Failed | TaskState::Cancelled | TaskState::Backed => Vote::Settled,
        TaskState::Skipped | TaskState::Removed => Vote::Withdrawn,
        TaskState::Aborted => Vote::Aborted,
        TaskState::Created | TaskState::Ready | TaskState::Running | TaskState::Waiting => {
            Vote::InFlight
        }
    }
}

/// What a rule decided for a parked container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    /// Not yet: keep waiting. An aborted child also waits — the abort walk
    /// owns the container's outcome (completing here would double-decide,
    /// exactly like the legacy `auto_complete` refusal).
    Wait,
    /// The rule decided the container completes. `withdraw` lists the
    /// counted children still in flight (ready/running/waiting): each is
    /// handed a `SkipTask`, so the vote is withdrawn and the record closes
    /// clean instead of parking strays under a decided container. A failure
    /// cancels nothing itself — its propagation is a dead-source cascade,
    /// which already skips the live children.
    Complete { withdraw: Vec<TaskId> },
    /// The threshold became unreachable: the container fails with the
    /// rule's own error pair.
    Fail { code: String, message: String },
}

/// The verdict one deciding package's request carries: the request's own
/// settled state *is* the verdict — completed = pass, failed = reject
/// (the rejection's error pair is the container's), skipped = defer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Verdict {
    /// The container completes.
    Complete,
    /// The container fails with the answer's own error pair.
    Fail { code: String, message: String },
    /// Defer: the request settles skipped; the next settlement asks again —
    /// and once every child settled with no request live, the engine asks
    /// once more (the client is the decider, so the re-ask loop is its own
    /// choice — a stuck client is a stuck container, exactly like an
    /// unanswered act).
    Defer,
}

/// Decide one parked container from its children's states.
///
/// `rule` is the container's declared rule (`None` is the original shape:
/// every child settles, none aborted). A delegated rule never decides by
/// count — the caller yields it to the shell ([`verdict`]).
pub(crate) fn decide(rule: Option<&crate::JoinRule>, children: &[(TaskId, TaskState)]) -> Decision {
    match rule {
        None | Some(crate::JoinRule::All) => decide_all(children),
        Some(rule @ (crate::JoinRule::Any | crate::JoinRule::Partial(_))) => {
            decide_threshold(rule, children)
        }
        Some(crate::JoinRule::Action { .. }) => Decision::Wait,
    }
}

/// The original shape: every child terminal, none aborted. A child failure
/// never decides here — the failed child's own error walk decides the chain.
fn decide_all(children: &[(TaskId, TaskState)]) -> Decision {
    for (_, state) in children {
        if !state.is_terminal() {
            return Decision::Wait;
        }
        if *state == TaskState::Aborted {
            return Decision::Wait;
        }
    }
    Decision::Complete {
        withdraw: Vec::new(),
    }
}

/// A threshold rule: complete on `K` completions, fail once `K` can no
/// longer be reached, wait otherwise.
///
/// A settled non-completion (`Failed`, `Cancelled`, `Backed`) is a vote that
/// can never pass — unlike `all`, the threshold rules do not fail the chain
/// on it (the error walk is gated for them); it only shrinks what the
/// remaining children can still reach.
fn decide_threshold(rule: &crate::JoinRule, children: &[(TaskId, TaskState)]) -> Decision {
    let mut completions = 0usize;
    let mut counted = 0usize;
    let mut inflight: Vec<&TaskId> = Vec::new();
    for (id, state) in children {
        match vote(*state) {
            Vote::Aborted => return Decision::Wait,
            Vote::Completion => {
                completions += 1;
                counted += 1;
            }
            Vote::Settled => counted += 1,
            Vote::Withdrawn => {}
            Vote::InFlight => {
                counted += 1;
                inflight.push(id);
            }
        }
    }
    let threshold = rule.threshold(counted);
    let unreachable = completions + inflight.len() < threshold;
    if completions < threshold && !unreachable {
        return Decision::Wait;
    }
    if unreachable {
        return Decision::Fail {
            code: "join_failed".to_string(),
            message: format!(
                "join '{rule}' unreachable: {completions} of {threshold} completions, {} of {counted} counted children settled",
                counted - inflight.len()
            ),
        };
    }
    // Only a completion cancels the stragglers itself. A `Created` child
    // never reaches the cancel list: `SkipTask` settles admitted executions
    // only (a container's children are admitted by its own fan-out, so the
    // case cannot arise; the filter keeps the plan valid if it ever does).
    Decision::Complete {
        withdraw: inflight
            .into_iter()
            .filter(|id| {
                matches!(
                    vote_in_flight_state(id, children),
                    Some(TaskState::Ready | TaskState::Running | TaskState::Waiting)
                )
            })
            .cloned()
            .collect(),
    }
}

/// The state of one child in the census (a `Created` child filtered from the
/// cancels; see [`decide_threshold`]).
fn vote_in_flight_state(id: &TaskId, children: &[(TaskId, TaskState)]) -> Option<TaskState> {
    children
        .iter()
        .find(|(child, _)| child == id)
        .map(|(_, state)| *state)
}

/// Map one settled request to the container's verdict. `None` is "nothing to
/// do": a skipped request whose container still holds other work waits for
/// the next settlement instead of re-asking now.
pub(crate) fn verdict(
    state: TaskState,
    error: Option<(String, String)>,
    live_work: bool,
) -> Option<Verdict> {
    match state {
        TaskState::Completed => Some(Verdict::Complete),
        TaskState::Failed => Some(Verdict::Fail {
            code: error
                .as_ref()
                .map(|(code, _)| code.clone())
                .unwrap_or_default(),
            message: error
                .as_ref()
                .map(|(_, message)| message.clone())
                .unwrap_or_default(),
        }),
        TaskState::Skipped if !live_work => Some(Verdict::Defer),
        _ => None,
    }
}
