use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskSnapshot {
    pub(crate) model_node: NodeId,
    pub(crate) parent: Option<TaskId>,
    pub(crate) predecessors: Vec<TaskId>,
    pub(crate) state: TaskState,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessSnapshot {
    pub(crate) process_id: String,
    pub(crate) version: ProcessVersion,
    pub(crate) tasks: HashMap<TaskId, TaskSnapshot>,
    pub(crate) variables: BTreeMap<(TaskId, String), String>,
    pub(crate) operations:
        HashMap<crate::scheduler::core::model::operation::OperationId, OperationRecord>,
    pub(crate) outbox: HashMap<
        crate::scheduler::core::outbox::records::OutboxId,
        crate::scheduler::core::outbox::records::OutboxRecord,
    >,
}

impl ProcessSnapshot {
    /// The next free operation sequence for one execution: operation ids are
    /// unique per (task, generation), and an execution that already ran operations
    /// (a fan-out child, a rewound generation) owns earlier sequences, so
    /// every newly seeded operation takes the one after the highest known.
    pub(crate) fn next_operation_sequence(&self, node: &str, generation: u64) -> u64 {
        self.operations
            .keys()
            .filter(|id| id.task.node.as_str() == node && id.task.generation == generation)
            .map(|id| id.sequence)
            .max()
            .map_or(0, |sequence| sequence + 1)
    }

    pub(crate) fn empty(process_id: impl Into<String>) -> Result<Self, CommitError> {
        let process_id = process_id.into();
        if process_id.trim().is_empty() {
            return Err(CommitError::EmptyProcessId);
        }
        Ok(Self {
            process_id,
            version: ProcessVersion {
                graph: 0,
                process: 0,
                operations: 0,
            },
            tasks: HashMap::new(),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        })
    }

    pub(crate) fn validate(&self) -> Result<(), CommitError> {
        if self.process_id.trim().is_empty() {
            return Err(CommitError::EmptyProcessId);
        }
        for (id, operation) in &self.operations {
            if id != &operation.id {
                return Err(CommitError::OperationIdentityMismatch {
                    key: id.clone(),
                    record: operation.id.clone(),
                });
            }
            if id.process_id != self.process_id {
                return Err(CommitError::ForeignOperation(id.clone()));
            }
        }
        for (id, record) in &self.outbox {
            if id != &record.id {
                return Err(CommitError::OutboxIdentityMismatch {
                    key: id.clone(),
                    record: record.id.clone(),
                });
            }
            if id.process_id != self.process_id {
                return Err(CommitError::ForeignOutbox(id.clone()));
            }
            record.validate().map_err(CommitError::InvalidOutbox)?;
        }
        for (id, task) in &self.tasks {
            if let Some(parent) = &task.parent
                && !self.tasks.contains_key(parent)
            {
                return Err(CommitError::TaskNotFound(parent.clone()));
            }
            let mut predecessors = HashSet::new();
            for predecessor in &task.predecessors {
                if predecessor == id || !predecessors.insert(predecessor.clone()) {
                    return Err(CommitError::InvalidTaskTopology {
                        task: id.clone(),
                        predecessor: predecessor.clone(),
                    });
                }
                if !self.tasks.contains_key(predecessor) {
                    return Err(CommitError::TaskNotFound(predecessor.clone()));
                }
            }
        }
        for (task, name) in self.variables.keys() {
            if !self.tasks.contains_key(task) {
                return Err(CommitError::TaskNotFound(task.clone()));
            }
            if name.trim().is_empty() {
                return Err(CommitError::EmptyVariableName);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommitReceipt {
    pub(crate) token: CommitToken,
    pub(crate) previous: ProcessVersion,
    pub(crate) current: ProcessVersion,
    pub(crate) new_operations: Vec<crate::scheduler::core::model::operation::OperationId>,
    pub(crate) new_outbox: Vec<OutboxId>,
}
