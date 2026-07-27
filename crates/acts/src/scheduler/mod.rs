mod context;
mod process;
mod queue;
mod runtime;
mod state;
mod tree;

#[cfg(test)]
mod tests;

pub use crate::Result;
pub use context::Context;
pub use process::{Process, Task};
pub use runtime::Runtime;
pub use state::TaskState;

#[allow(unused_imports)]
pub use tree::{Node, NodeContent, NodeData, NodeKind, NodeTree};

pub trait ActTask: Clone + Send {
    fn init(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn run(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn next(&self, _ctx: &Context) -> Result<bool> {
        Ok(false)
    }

    fn review(&self, _ctx: &Context) -> Result<bool> {
        Ok(true)
    }

    fn on_error(&self, ctx: &Context) -> Result<()> {
        ctx.emit_error()
    }

    fn on_timeout(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}
