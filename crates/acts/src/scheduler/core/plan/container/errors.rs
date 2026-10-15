use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContainerError {
    WrongProcess,
    ExecutionNotFound(ExecutionId),
    SourceNotContainer {
        source: ExecutionId,
        state: ExecutionState,
    },
    SourceOperationTargetsDifferentExecution,
    InvalidSourcePark(ExecutionState),
    EmptyChildTaskId,
    ForwardSibling(usize),
    ReadmitNotCreated {
        execution: ExecutionId,
        state: ExecutionState,
    },
    GraphVersionConflict {
        graph: u64,
        snapshot: u64,
    },
    GenerationOverflow,
    SequenceOverflow,
    InvalidPlan(String),
}
impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "container error: {self:?}")
    }
}
impl std::error::Error for ContainerError {}
