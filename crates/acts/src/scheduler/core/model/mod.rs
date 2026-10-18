//! Domain model: durable facts and their transitions.

pub(crate) mod clock;
pub(crate) mod effect;
pub(crate) mod legacy_task_state;
pub(crate) mod model_graph;
pub(crate) mod operation;
pub(crate) mod task_graph;
pub(crate) mod transition_plan;
pub(crate) mod view;
