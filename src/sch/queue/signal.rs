use std::sync::Arc;

use crate::sch::Task;

#[derive(Clone)]
pub enum Signal {
    Terminal,
    Task(Arc<Task>),
}

impl std::fmt::Debug for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal => write!(f, "Terminal"),
            Self::Task(t) => f
                .debug_tuple("Task")
                .field(&t.node.kind())
                .field(&t.node.content.name())
                .field(&t.id)
                .finish(),
        }
    }
}
