//! provides an acts client channel for acts-server

#![doc = include_str!("../README.md")]
// `tonic::Status` (~176 bytes) is the crate's gRPC error type by design; boxing
// it would break the public API, so suppress the large-Result lint crate-wide.
#![allow(clippy::result_large_err)]

mod action_result;
mod channel;
#[cfg(test)]
mod tests;
mod utils;
mod vars;

pub mod model;
pub use action_result::ActionResult;
pub use channel::{
    ActsChannel, ActsOptions, Auth, AuthChannel, SessionTokens, Subscription, SubscriptionError,
};
pub use vars::Vars;

// The wire protocol is generated in `acts-proto`, which both this client and
// the server plugin implement their half of; re-exported here so a caller that
// reaches for the client keeps finding the messages and the service stubs
// under this crate, as it always has.
pub use acts_proto::{
    Message, MessageOptions, acts_service_client, acts_service_server, create_seq,
};
