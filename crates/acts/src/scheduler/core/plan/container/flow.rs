use super::*;

/// The private variable an execution carries while it decides a caught error
/// (legacy `Sign::ERROR` on the failed step) and its CATCH children carry
/// (legacy `TASK_SIGN = CATCH`). Both roles share one key and two values: the
/// sign is durable plan state, so a replay — or the catch join — reads the
/// decision from the aggregate instead of from the declaring spec.
pub(crate) const SIGN_VAR: &str = "__sign";
/// A child the failed step dispatched as a CATCH handler.
///
/// The *role* fact: what this execution is to its parent. It is a separate axis
/// from [`FLOW_VAR`] (who decides this execution next), and the two must stay
/// separate keys — a handler is often itself a container, and its own fan-out
/// would otherwise overwrite the role its parent's join reads back from it.
pub(crate) const SIGN_CATCH: &str = "catch";

/// The private variable holding which decision flow an execution is in, when it
/// is not simply in its own act's flow (the flow fact's *absence*).
///
/// Two values, one axis:
///
/// - [`FLOW_CHILDREN`]: the execution admitted children and is `Running` while
///   they settle; the plain join ([`ContainerPlanner::plan_join`]) decides it.
/// - [`FLOW_CATCHES`]: its failure is durable and it dispatched its `catches`;
///   the catch join ([`ContainerPlanner::plan_catch_join`]) decides it — and
///   only that join, so a plain container join can never mistake a failed step
///   for one waiting on its children (or a child's settlement for a completed
///   step's).
///
/// Both values are needed as *durable* facts rather than derived from the
/// state: after the park moved to `Running`, "`Running` with children" means
/// either flow, and which one decides the execution is exactly the question a
/// replay has to answer.
///
/// `timeout` is deliberately **not** a value here. A timer runs *in parallel*
/// to the act's own flow (it fires while the act executes or waits), so a
/// single-valued flow key could not hold it without losing the act flow; the
/// accurate expression of "a timeout is pending" is the timer operation itself
/// ([`crate::scheduler::core::model::operation::OperationKind::DispatchTimeout`]), which holds the cadence and is
/// re-woken by the recovery index.
pub(crate) const FLOW_VAR: &str = "__flow";
/// The value of [`FLOW_VAR`] on an execution waiting for the children it
/// admitted.
pub(crate) const FLOW_CHILDREN: &str = "children";
/// The value of [`FLOW_VAR`] on a failed execution deciding its `catches`.
pub(crate) const FLOW_CATCHES: &str = "catches";

/// Which decision flow one execution is in, beyond its own act's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionFlow {
    /// Waiting for the children it admitted: the plain join decides it.
    Children,
    /// Deciding its `catches`: the catch join decides it.
    Catches,
}

/// The flow fact ([`FLOW_VAR`]) of one execution, if any. `None` means the
/// execution is in its own act's flow — `Running` with it in flight, `Waiting`
/// for a client, or not started yet.
pub(crate) fn execution_flow(
    snapshot: &ProcessSnapshot,
    execution: &ExecutionId,
) -> Option<ExecutionFlow> {
    match snapshot
        .variables
        .get(&(execution.clone(), FLOW_VAR.to_string()))
        .map(String::as_str)
    {
        Some(FLOW_CHILDREN) => Some(ExecutionFlow::Children),
        Some(FLOW_CATCHES) => Some(ExecutionFlow::Catches),
        _ => None,
    }
}

/// Whether one execution is parked in `Running` for the children it admitted.
pub(crate) fn parked_for_children(snapshot: &ProcessSnapshot, execution: &ExecutionId) -> bool {
    execution_flow(snapshot, execution) == Some(ExecutionFlow::Children)
}

/// Whether one execution is deciding its `catches` (the legacy `Sign::ERROR`
/// on the failed step).
pub(crate) fn deciding_catches(snapshot: &ProcessSnapshot, execution: &ExecutionId) -> bool {
    execution_flow(snapshot, execution) == Some(ExecutionFlow::Catches)
}

/// The sign fact ([`SIGN_VAR`]) of one execution, if any.
pub(crate) fn execution_sign<'a>(
    snapshot: &'a ProcessSnapshot,
    execution: &ExecutionId,
) -> Option<&'a str> {
    snapshot
        .variables
        .get(&(execution.clone(), SIGN_VAR.to_string()))
        .map(String::as_str)
}
