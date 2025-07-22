mod act;
mod branch;
mod info;
mod step;
mod var;
mod workflow;

#[cfg(test)]
mod tests;

pub use act::{Act, Catch, Retry, Timeout, TimeoutLimit};
pub use branch::Branch;
pub use info::{EventInfo, MessageInfo, ModelInfo, PackageInfo, ProcInfo, TaskInfo};
pub use step::Step;
pub use var::{ActSchema, Variant, VariantTypes, Vars};
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}
