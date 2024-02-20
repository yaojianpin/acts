use crate::sch::{Proc, Task};
use std::sync::Arc;

#[derive(Clone)]
pub enum Signal {
    Terminal,
    Proc(Arc<Proc>),
    Task(Task),
}

impl std::fmt::Debug for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal => write!(f, "Terminal"),
            Self::Proc(p) => f.debug_tuple("Proc").field(&p.id()).finish(),
            Self::Task(t) => f
                .debug_tuple("Task")
                .field(&t.node.kind())
                .field(&t.node.content.name())
                .field(&t.id)
                .finish(),
        }
    }
}
