mod act;
mod branch;
mod cand;
mod info;
mod job;
mod state;
mod step;
mod workflow;

#[cfg(test)]
mod tests;

pub mod builder;

pub use act::{Act, ActAlias, ActCatch, ActFor};
pub use branch::Branch;
pub use cand::{Candidate, Operation, OrgAdapter, RoleAdapter};
pub use info::{ModelInfo, ProcInfo, TaskInfo};
pub use job::Job;
pub use state::{ActionResult, WorkflowState};
pub use step::Step;
pub use workflow::{Workflow, WorkflowAction};

pub trait ModelBase {
    fn id(&self) -> &str;
}
