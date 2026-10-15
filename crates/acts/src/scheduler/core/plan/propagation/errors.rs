use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PropagationError {
    WrongAggregate,
    GraphVersionConflict {
        graph: u64,
        snapshot: u64,
    },
    SourceOperationTargetsDifferentExecution,
    ExecutionNotInGraph(ExecutionId),
    ExecutionNotFound(ExecutionId),
    SourceNotTerminal {
        execution: ExecutionId,
        state: ExecutionState,
    },
    /// A step's `next` names a node the run never instantiated (the model and
    /// the seeded graph disagree). Scheduling nothing would end the flow
    /// silently, so the hop refuses instead.
    FlowTargetNotInstantiated(String),
    EmptyFlowTarget,
    GenerationOverflow,
    InvalidPlan(String),
}
impl fmt::Display for PropagationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "propagation error: {self:?}")
    }
}
impl std::error::Error for PropagationError {}
