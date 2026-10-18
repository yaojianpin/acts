//! The projection view: the persisted node shapes (`NodeData`/`NodeContent`)
//! a process's rows carry, derived straight from the declaring model.

mod build;
mod node;

pub(crate) use build::assign_ids;
pub(crate) use node::NodeContent;
pub use node::{NodeData, NodeKind};
