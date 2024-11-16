mod act;
mod branch;
mod info;
mod output;
mod step;
mod vars;
mod workflow;

#[cfg(test)]
mod tests;

pub use act::{Act, ActFn, Block, Call, Catch, Chain, Do, Each, If, Irq, Msg, Pack, Timeout};
pub use branch::Branch;
pub use info::{MessageInfo, ModelInfo, PackageInfo, ProcInfo, TaskInfo};
pub use output::{Output, OutputType, Outputs};
pub use step::Step;
pub use vars::Vars;
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}

pub trait StmtBuild<T> {
    fn add(self, s: T) -> Self;
    fn with<F: Fn(T) -> T>(self, build: F) -> Self
    where
        T: Default;
}
