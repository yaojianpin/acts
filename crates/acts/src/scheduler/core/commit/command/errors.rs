use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandError {
    WrongProcess {
        expected: String,
        actual: String,
    },
    ForeignSourceOperation(OperationId),
    SourceOperationTargetsDifferentTask,
    TaskNotFound(TaskId),
    ActionNotAllowed {
        state: TaskState,
        action: ActionKind,
    },
    EmptyInputName,
    DuplicateInput(String),
    ReservedErrorInput,
    SequenceOverflow,
    InvalidPlan(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "command error: {self:?}")
    }
}
impl std::error::Error for CommandError {}
