mod act;
mod branch;
mod job;
mod proc_info;
mod state;
mod step;
mod workflow;

pub mod builder;

pub use act::Act;
pub use branch::Branch;
pub use job::Job;
pub use proc_info::ProcInfo;
pub use state::State;
pub use step::{Action, Step, Subject};
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}
