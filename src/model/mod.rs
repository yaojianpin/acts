mod act;
mod action_result;
mod branch;
mod info;
mod step;
mod vars;
mod workflow;

#[cfg(test)]
mod tests;

pub use act::{Act, Block, Call, Chain, Cmd, Each, If, Msg, Pack, Req};
pub use action_result::ActionResult;
pub use branch::Branch;
pub use info::{ModelInfo, PackageInfo, ProcInfo, TaskInfo};
pub use step::{Catch, Step, Timeout};
pub use vars::Vars;
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}

pub trait StmtBuild<T> {
    fn add(self, s: T) -> Self;
}
