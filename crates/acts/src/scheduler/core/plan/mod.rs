//! Pure planners: every state decision the kernel makes, given a snapshot.

pub(crate) mod branch;
pub(crate) mod concurrency;
pub(crate) mod container;
pub(crate) mod cron;
pub(crate) mod generation;
pub(crate) mod propagation;
pub(crate) mod task_admission;
pub(crate) mod timeout;
pub(crate) mod upward;
