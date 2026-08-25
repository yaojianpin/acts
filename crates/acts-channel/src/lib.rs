//! provides an acts client channel for acts-server

#![doc = include_str!("../README.md")]
// `tonic::Status` (~176 bytes) is the crate's gRPC error type by design; boxing
// it would break the public API, so suppress the large-Result lint crate-wide.
#![allow(clippy::result_large_err)]

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
