mod branch;
mod candidate;
mod info;
mod job;
mod state;
mod step;
mod workflow;

#[cfg(test)]
mod tests;

pub mod builder;

pub use branch::Branch;
pub use candidate::{Candidate, Operation, OrgAdapter, RoleAdapter};
pub use info::{ActInfo, ModelInfo, ProcInfo, TaskInfo};
pub use job::Job;
pub use state::{ActionState, WorkflowState};
pub use step::{Step, Subject};
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}
