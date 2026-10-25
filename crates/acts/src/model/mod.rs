mod branch;
mod catch;
mod info;
mod step;
mod timeout;
mod trigger;
mod var;
mod workflow;

#[cfg(test)]
mod tests;

pub use branch::Branch;
pub use catch::Catch;
pub use info::{
    EventInfo, GraphNode, MessageInfo, ModelInfo, PackageInfo, ProcInfo, ProcessGraph, TaskInfo,
};
pub use step::Step;
pub use timeout::{Timeout, TimeoutLimit, TimeoutUnit};
pub use trigger::{
    TRIGGER_CHAT, TRIGGER_HOOK, TRIGGER_MANUAL, TRIGGER_SCHEDULE, Trigger, TriggerKind,
};
pub use var::{ActSchema, Variant, VariantTypes, Vars};
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}
