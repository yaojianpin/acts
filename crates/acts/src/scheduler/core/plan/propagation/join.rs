use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum JoinReadiness {
    Ready,
    Waiting,
    SkipCascade {
        predecessor: ExecutionId,
        state: ExecutionState,
    },
}

pub(super) fn readiness(
    graph: &ExecutionGraph,
    snapshot: &AggregateSnapshot,
    successor: &ExecutionId,
    policy: JoinPolicy,
) -> Result<JoinReadiness, PropagationError> {
    let predecessors = graph.predecessors(successor);
    if predecessors.is_empty() {
        return Ok(JoinReadiness::Ready);
    }
    let mut waiting = false;
    for predecessor in predecessors {
        let execution = snapshot
            .executions
            .get(predecessor)
            .ok_or_else(|| PropagationError::ExecutionNotFound(predecessor.clone()))?;
        let state = execution.state;
        if !state.is_terminal() {
            waiting = true;
            continue;
        }
        // A step its own `if`/`while` guard skipped is the legacy
        // fall-through, not a dead path: its successor runs (the guard said
        // "not now", not "everything below is dead").
        if state == ExecutionState::Skipped && is_guard_skip(snapshot, predecessor) {
            continue;
        }
        if policy == JoinPolicy::AllSucceeded && !is_success(state) {
            // A dead terminal predecessor (cancelled/failed/aborted/skipped)
            // is decisive: the successor is skipped even if other
            // predecessors are still in flight — the flow is already over on
            // this path.
            return Ok(JoinReadiness::SkipCascade {
                predecessor: predecessor.clone(),
                state,
            });
        }
    }
    Ok(if waiting {
        JoinReadiness::Waiting
    } else {
        JoinReadiness::Ready
    })
}
