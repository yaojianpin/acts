mod cache;
mod context;
mod proc;
mod queue;
mod scher;
mod state;
mod tree;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use core::clone::Clone;

pub use context::Context;
pub use proc::{Act, ActKind, Proc, Task};
pub use scher::Scheduler;
pub use state::TaskState;
pub use tree::{Node, NodeData, NodeKind, NodeTree};

#[async_trait]
pub trait ActTask: Clone + Send {
    fn prepare(&self, _ctx: &Context) {}
    fn run(&self, ctx: &Context);
    fn post(&self, _ctx: &Context) {}
}
