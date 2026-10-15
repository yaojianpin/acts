use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GenerationError {
    WrongAggregate,
    ExecutionNotFound(ExecutionId),
    SourceNotReenterable {
        source: ExecutionId,
        state: ExecutionState,
    },
    SourceOperationTargetsDifferentExecution,
    EmptyTargetTaskId,
    TargetNeverInstantiated(String),
    /// A Back named a `to` step that is not on the source's path: the redo
    /// closure from that step would not carry the backed source.
    SourceOutsideRedoClosure {
        source: ExecutionId,
        target: String,
    },
    GenerationOverflow,
    InvalidPlan(String),
}
impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "generation error: {self:?}")
    }
}
impl std::error::Error for GenerationError {}
