use super::*;

/// What a fan-out child is admitted as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerChildPlan {
    /// Runs now: created `Ready` (or readmitted from `Created`) with an
    /// `ExecuteTask` operation.
    Entry,
    /// Not selected: created `Ready` with a `SkipTask` operation, so the
    /// parity floor keeps one terminal execution per model node (the legacy
    /// engine keeps the skipped branch task).
    Skip,
    /// A chain successor: created `Created`; ordinary propagation admits it
    /// when its predecessor settles. An execution that already exists (the
    /// root's seeded chain, or a rewind's recreation) is left exactly as it
    /// is — the fan-out never readmits one, because the admission would hand
    /// it `Ready` with no operation to run.
    Queued,
}

/// One child execution to admit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerChild {
    pub(crate) task_id: String,
    pub(crate) model_node: NodeId,
    pub(crate) plan: ContainerChildPlan,
    /// Edges of the created execution. `Source` is the fanning-out
    /// container; `Sibling(i)` the i-th child of the same command, so the
    /// host can chain children whose generations only the planner computes.
    pub(crate) predecessors: Vec<ContainerPredecessor>,
    /// Declared model vars, seeded as this execution's own scope (JSON
    /// strings, the aggregate's variable representation).
    pub(crate) vars: Vec<(String, String)>,
    pub(crate) auto: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerPredecessor {
    Source,
    Sibling(usize),
}
