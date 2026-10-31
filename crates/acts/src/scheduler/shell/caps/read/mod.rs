//! The read capability: the legacy row collections projected from the
//! aggregate (`projection`) and the row-based read APIs behind `proc:get`
//! (`read_model`).

#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod projection;
pub(crate) mod read_model;

pub(crate) use projection::{ProjectionMetrics, RowProjector};
