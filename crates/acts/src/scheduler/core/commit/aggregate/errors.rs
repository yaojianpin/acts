use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommitError {
    EmptyProcessId,
    #[cfg(test)]
    ProcessAlreadyExists(String),
    ProcessNotFound(String),
    InvalidPlan(String),
    VersionConflict {
        expected: ProcessVersion,
        actual: ProcessVersion,
    },
    TaskNotFound(TaskId),
    TaskAlreadyExists(TaskId),
    GenerationNotMonotonic(TaskId),
    TaskConflict {
        task: TaskId,
        expected_revision: u64,
        actual_revision: u64,
        expected_state: TaskState,
        actual_state: TaskState,
    },
    OperationNotFound(crate::scheduler::core::model::operation::OperationId),
    OperationIdentityMismatch {
        key: crate::scheduler::core::model::operation::OperationId,
        record: crate::scheduler::core::model::operation::OperationId,
    },
    ForeignOperation(crate::scheduler::core::model::operation::OperationId),
    InvalidTaskTopology {
        task: TaskId,
        predecessor: TaskId,
    },
    EmptyVariableName,
    OperationRevisionConflict {
        operation: crate::scheduler::core::model::operation::OperationId,
        expected: u64,
        actual: u64,
    },
    OperationAlreadyExists(crate::scheduler::core::model::operation::OperationId),
    OutboxAlreadyExists(String),
    OutboxIdentityMismatch {
        key: OutboxId,
        record: OutboxId,
    },
    ForeignOutbox(OutboxId),
    InvalidOutbox(crate::scheduler::core::outbox::records::OutboxError),
    OutboxEncoding(String),
    RevisionOverflow,
    InvalidPageLimit,
    InvalidOperationState(OperationState),
    CorruptProcess(String),
}
impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "commit error: {self:?}")
    }
}
impl std::error::Error for CommitError {}
