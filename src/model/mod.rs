mod act;
mod branch;
mod info;
mod output;
mod step;
mod vars;
mod workflow;

#[cfg(test)]
mod tests;

pub use act::{Act, Catch, Retry, Timeout, TimeoutLimit};
pub use branch::Branch;
pub use info::{MessageInfo, ModelInfo, PackageInfo, ProcInfo, TaskInfo};
pub use output::{Output, OutputType, Outputs};
pub use step::Step;
pub use vars::Vars;
pub use workflow::Workflow;

use serde::{Deserialize, Serialize};
pub trait ModelBase {
    fn id(&self) -> &str;
}

pub trait StmtBuild<T> {
    fn add(self, s: T) -> Self;
    fn with<F: Fn(T) -> T>(self, build: F) -> Self
    where
        T: Default;
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    strum::AsRefStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ActEvent {
    /// trigger when task created
    #[default]
    Created,

    /// trigger when task completed
    Completed,

    /// before act executing
    BeforeUpdate,

    /// trigger when act to update the state
    /// based on Step node
    Updated,

    /// trigger when step move to next
    /// based on Workflow node
    Step,
}
