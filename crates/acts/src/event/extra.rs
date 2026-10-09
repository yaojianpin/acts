use std::sync::Arc;

use crate::scheduler::Process;

#[derive(Debug, Clone, Default)]
pub struct TaskExtra {
    pub emit_message: bool,
    /// The task's owning process: the handler needs it to build the task's
    /// message (`Task::create_message`), and the task no longer carries the
    /// handle itself. Always set by the engine's task-event emit.
    pub proc: Option<Arc<Process>>,
}
