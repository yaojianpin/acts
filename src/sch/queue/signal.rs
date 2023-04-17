use crate::{
    event::UserMessage,
    sch::{Proc, Task},
};
use std::sync::Arc;

#[derive(Clone)]
pub enum Signal {
    Terminal,
    Proc(Arc<Proc>),
    Task(Task),
    Message(UserMessage),
}

impl std::fmt::Debug for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal => write!(f, "Terminal"),
            Self::Proc(arg0) => f.debug_tuple("Proc").field(&arg0.pid()).finish(),
            Self::Task(arg0) => f.debug_tuple("Task").field(&arg0.tid).finish(),
            Self::Message(arg0) => f.debug_tuple("Message").field(arg0).finish(),
        }
    }
}
