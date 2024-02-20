mod act;
mod branch;
mod cand;
mod info;
mod state;
mod step;
mod vars;
mod workflow;

#[cfg(test)]
mod tests;

pub use act::{Act, Chain, Cmd, Each, If, Msg, Package, Req, Use};
pub use branch::Branch;
pub use cand::{Candidate, Operation, OrgAdapter, RoleAdapter};
pub use info::{ModelInfo, ProcInfo, TaskInfo};
pub use state::{ActionResult, WorkflowState};
pub use step::{Catch, Step, Timeout};
pub use vars::Vars;
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}

pub trait StmtBuild<T> {
    fn add(self, s: T) -> Self;
}
