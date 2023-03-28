mod cache;
mod consts;
mod context;
mod event;
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
pub use event::{ActionOptions, Event, EventAction, EventData, Message, UserMessage};
pub use proc::{Matcher, Proc, Task};
pub use scher::Scheduler;
pub use state::TaskState;
pub use tree::{Node, NodeData, NodeKind, NodeTree};

#[async_trait]
pub trait ActTask: Clone + Send {
    fn prepare(&self, _ctx: &Context) {}
    fn run(&self, ctx: &Context);
    fn post(&self, _ctx: &Context) {}
}

pub trait ActId: Clone {
    fn tid(&self) -> String;
}

pub trait ActState: Clone + Send {
    fn set_state(&self, state: &TaskState);
    fn state(&self) -> TaskState;
}

pub trait ActTime: Clone + Send {
    fn get_state_time(&self) -> u64;
    fn get_end_time(&self) -> u64;
}
