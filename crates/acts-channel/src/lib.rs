//! provides an acts client channel for acts-server

#![doc = include_str!("../README.md")]

include!("../proto/acts.grpc.rs");

mod action_result;
mod channel;
#[cfg(test)]
mod tests;
mod utils;
mod vars;

pub mod model;
pub use action_result::ActionResult;
pub use channel::{ActsChannel, ActsOptions};
pub use utils::create_seq;
pub use vars::Vars;
