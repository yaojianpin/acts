mod branch;
mod catch;
mod info;
mod join;
mod owner;
mod step;
mod timeout;
mod trigger;
mod validation;
mod var;
mod workflow;

#[cfg(test)]
mod tests;

pub use branch::Branch;
pub use catch::Catch;
pub use info::{
    EventInfo, GraphNode, MessageInfo, ModelInfo, PackageInfo, ProcInfo, ProcessGraph, TaskInfo,
};
pub use join::{JoinRule, PartialThreshold};
pub(crate) use owner::participants_from_json;
pub use owner::{
    ContextVar, Duty, OwnerExpr, Participant, Relation, RelationName, RelationRef, ResourceRef,
    VarType,
};
pub use step::{Assign, Step};
pub use timeout::{Timeout, TimeoutLimit, TimeoutUnit};
pub use trigger::{
    TRIGGER_CHAT, TRIGGER_HOOK, TRIGGER_MANUAL, TRIGGER_SCHEDULE, Trigger, TriggerKind,
};
pub use validation::ValidationRule;
pub use var::{ActSchema, Variant, VariantTypes, Vars};
pub use workflow::Workflow;

pub trait ModelBase {
    fn id(&self) -> &str;
}
