//! Pure slot policy for the runs a driver may have in flight at once.
//!
//! `Config::cache_cap` bounded the legacy engine's *resident object set*: that
//! engine holds one in-memory process per live run, so the number was a memory
//! bound. The kernel holds no resident object — every fact lives in the
//! aggregate document — so the same configured number is redefined here as the
//! bound on runs that are *in flight* at once: a start beyond the cap parks,
//! and a run that settles frees its slot for the parked run that has waited
//! longest. This module is the whole rule; the shell owns the set it bounds
//! (its driver's active set) and the seed a parked run gets.
//!
//! Nothing here reads a clock, a store or a layer above this one.

use crate::scheduler::core::model::task_graph::TaskState;
use crate::scheduler::core::plan::ActPlan;

/// The creation stamp of one run, written into its seed on the root scope.
///
/// A freed slot goes to the parked run that has waited longest, and the legacy
/// engine ordered its parked rows by `timestamp` ascending; the aggregate
/// keeps no wall-clock stamp of its own, so the run carries the one fact that
/// order needs. Like the cost start of a timed-out execution
/// ([`STARTED_VAR`](super::timeout::STARTED_VAR)) it is a kernel fact: it stays
/// `__`-private, so it never reaches a scope a model reads, and it is written
/// by the seed, not by a plan.
pub(crate) const CREATED_VAR: &str = "__created";

/// What the driver does with a run that does not hold a slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Admission {
    /// Let the run operation: it holds a slot, or needs none.
    Admit,
    /// Hold the run back until a slot frees.
    Park,
}

/// The plan of one run's slot decision: whether the driver lets it operation now or
/// holds it back until a slot frees.
pub(crate) struct Admit;

impl ActPlan for Admit {
    type Reading<'a> = ();
    type Command<'a> = (usize, usize, TaskState);
    type Output = Admission;
    type Error = std::convert::Infallible;

    /// The slot decision for one run:
    ///
    /// - `cap` is the configured bound on runs in flight at once;
    /// - `active` is how many runs hold a slot right now;
    /// - `state` is the run's root execution state.
    ///
    /// A run whose root already reached a terminal state is admitted without a
    /// slot: nothing is left to bound, and parking it would let a finished run's
    /// own bookkeeping (a timer operation still open) wait behind the cap. Every
    /// other run takes one of the `cap` slots — a cap of zero admits none, exactly
    /// like an empty legacy resident set — or parks.
    fn plan(_reading: (), command: &Self::Command<'_>) -> Result<Self::Output, Self::Error> {
        let (cap, active, state) = *command;
        Ok(if state.is_terminal() || active < cap {
            Admission::Admit
        } else {
            Admission::Park
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::plan::infallible;

    /// The two states a run can ask for a slot in: a fresh start seeds its
    /// root `Ready`, a parked one is seeded `Created`.
    const ASKING: [TaskState; 2] = [TaskState::Created, TaskState::Ready];

    #[test]
    fn a_free_slot_admits_and_a_full_set_parks() {
        for state in ASKING {
            assert_eq!(
                infallible(Admit::plan((), &(2, 0, state))),
                Admission::Admit
            );
            assert_eq!(
                infallible(Admit::plan((), &(2, 1, state))),
                Admission::Admit
            );
            assert_eq!(infallible(Admit::plan((), &(2, 2, state))), Admission::Park);
            // A set beyond the cap (a cap lowered under a running driver, or a
            // concurrent start) must not admit anything more.
            assert_eq!(infallible(Admit::plan((), &(1, 4, state))), Admission::Park);
        }
    }

    #[test]
    fn a_settled_root_never_waits_for_a_slot() {
        for state in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Aborted,
            TaskState::Skipped,
            TaskState::Removed,
        ] {
            assert_eq!(
                infallible(Admit::plan((), &(0, 0, state))),
                Admission::Admit
            );
            assert_eq!(
                infallible(Admit::plan((), &(1, 9, state))),
                Admission::Admit
            );
        }
    }

    #[test]
    fn a_zero_cap_admits_nothing_with_work_left() {
        for state in ASKING {
            assert_eq!(infallible(Admit::plan((), &(0, 0, state))), Admission::Park);
        }
    }
}
