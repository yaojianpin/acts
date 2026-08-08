mod context;
mod process;
mod queue;
mod runtime;
mod state;
mod tree;

#[cfg(test)]
mod tests;

use core::fmt;

pub use crate::Result;
pub use context::Context;
pub use process::{Process, Task};
pub use runtime::Runtime;
pub use state::TaskState;

#[allow(unused_imports)]
pub use tree::{Node, NodeContent, NodeData, NodeKind, NodeTree};

pub enum NextAction {
    Continue,
    Parent,
    Stop,
}

pub trait ActTask: Clone + Send {
    fn init(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn run(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn next(&self, _ctx: &Context) -> Result<NextAction> {
        Ok(NextAction::Parent)
    }

    fn on_error(&self, ctx: &Context) -> Result<()> {
        ctx.emit_error()
    }

    fn on_timeout(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}

impl fmt::Display for NextAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NextAction::Continue => write!(f, "continue"),
            NextAction::Parent => write!(f, "parent"),
            NextAction::Stop => write!(f, "stop"),
        }
    }
}

impl NextAction {
    #[allow(dead_code)]
    pub fn is_continue(&self) -> bool {
        matches!(self, NextAction::Continue)
    }

    pub fn is_parent(&self) -> bool {
        matches!(self, NextAction::Parent)
    }
}
