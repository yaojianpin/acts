use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UpwardError {
    WrongProcess,
    TaskNotFound(TaskId),
    UnsupportedTarget(TaskState),
    SourceNotDecided {
        source: TaskId,
        state: TaskState,
        to: TaskState,
    },
}
impl fmt::Display for UpwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "upward walk error: {self:?}")
    }
}
impl std::error::Error for UpwardError {}
