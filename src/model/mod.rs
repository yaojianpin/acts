mod act;
mod branch;
mod info;
mod job;
mod state;
mod step;
mod workflow;

pub mod builder;

pub use act::Act;
pub use branch::Branch;
pub use info::{ModelInfo, ProcInfo, TaskInfo};
pub use job::Job;
pub use state::State;
pub use step::{Action, Step, Subject};
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}
