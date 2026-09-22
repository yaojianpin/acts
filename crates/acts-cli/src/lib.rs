//! An interactive gRPC client for an acts-server, as a library so the command
//! layer is drivable by tests as well as by the `acts-cli` REPL.
//!
//! Nothing here panics on what a server or a user says: a failed round trip is
//! an error naming the action the user typed, a response without the payload
//! the action promised is an error naming the action, and a command line that
//! does not parse is an error carrying clap's own diagnostic. Only the REPL
//! decides what to print and whether to exit.

pub mod cli;
pub mod client;
pub mod cmd;
pub mod util;
