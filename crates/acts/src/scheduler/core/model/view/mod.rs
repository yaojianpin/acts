//! The projection view: the workflow's node tree, as a process's rows
//! materialize it (`shell/projection.rs`) and as the act scope walks it.

mod build;
mod node;
mod node_tree;

pub use build::dyn_build_act;
#[cfg(test)]
pub use node::NodeContent;
pub use node::{Node, NodeData, NodeKind};
pub use node_tree::NodeTree;
