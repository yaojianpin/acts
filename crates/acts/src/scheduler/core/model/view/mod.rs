//! The projection view: the workflow's node tree, as a process's rows
//! materialize it (`shell/projection.rs`) and as the act scope walks it.

mod build;
mod node;
mod node_tree;
mod visit;

pub use build::dyn_build_act;
#[allow(unused_imports)]
pub use node::{Node, NodeContent, NodeData, NodeKind, NodeOutputKind};
pub use node_tree::NodeTree;
