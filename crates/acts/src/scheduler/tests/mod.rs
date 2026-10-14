//! The runtime's cross-layer suite: the reliability matrix.
//!
//! It drives runs through `Engine` (the crate's public handle) and observes the
//! kernel at its own paths — the aggregate, the execution/operation state, the
//! outbox and the row keys — because what it pins is the *durability* contract
//! of that machinery, not any one layer. Living here, beside the layers, is what
//! keeps the module's outward surface (`mod.rs`) to what a serving path needs.

mod reliability;
