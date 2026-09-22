//! the generated bindings of the acts gRPC protocol

#![doc = include_str!("../README.md")]
// The crate's error type is `tonic::Status` (~176 bytes) by design; boxing it
// would break the generated service trait, so suppress the large-Result lint
// crate-wide.
#![allow(clippy::result_large_err)]

include!("../proto/acts.grpc.rs");

mod utils;

pub use utils::create_seq;
