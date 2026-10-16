use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContainerError {
    WrongProcess,
    TaskNotFound(TaskId),
    SourceNotContainer { source: TaskId, state: TaskState },
    SourceOperationTargetsDifferentTask,
    InvalidSourcePark(TaskState),
    EmptyChildTaskId,
    ForwardSibling(usize),
    ReadmitNotCreated { task: TaskId, state: TaskState },
    GraphVersionConflict { graph: u64, snapshot: u64 },
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
