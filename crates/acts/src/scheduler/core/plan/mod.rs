//! Pure planners: every state decision the kernel makes, given a snapshot.
//!
//! Every one of them is *a type* implementing [`ActPlan`]: an operation reads the
//! authoritative snapshot (and, where the decision's shape lives in the edges,
//! the graph rebuilt from it), hands the plan that reading plus the command it
//! read off it, and gets its output back. Nothing above this layer names a
//! concrete planning function, and nothing here names a layer above it: a plan
//! is pure policy over the aggregate's own facts.

pub(crate) mod branch;
pub(crate) mod concurrency;
pub(crate) mod container;
pub(crate) mod cron;
pub(crate) mod generation;
pub(crate) mod propagation;
pub(crate) mod task_admission;
pub(crate) mod timeout;
pub(crate) mod upward;

/// One plan kind: a type that answers "what comes next" from the authoritative
/// facts.
///
/// The contract is deliberately thin — read, command, output — because the
/// plans differ in what they read and what they answer with, not in how they
/// are called:
///
/// - [`Reading`](ActPlan::Reading) is what the plan reads: the snapshot, plus the
///   graph where the decision's shape lives in the edges; `()` for a plan whose
///   whole input is its command.
/// - [`Command`](ActPlan::Command) is what the caller read off that reading, in
///   the plan's own vocabulary (mostly the `*Command` DTO beside it).
/// - [`Output`](ActPlan::Output) is the plan: a whole durable transition, a
///   verdict, or the fragment its caller folds into the operation's plan.
/// - [`Error`](ActPlan::Error) is the plan's own refusal — never an engine error:
///   the caller maps it where it calls (see the operation drivers above this layer).
///
/// A plan that cannot refuse still returns a `Result` (see [`infallible`]): one
/// contract for the whole layer, not one for what fails and one for what
/// answers.
pub(crate) trait ActPlan {
    /// What this plan reads.
    type Reading<'a>;
    /// What the caller read for this plan.
    type Command<'a>;
    /// What the plan produces.
    type Output;
    /// What it refuses with.
    type Error: std::fmt::Display;

    /// ActPlan from one reading and one command. The reading is a value (a
    /// reference, or a pair of them) — the plan does not hold it.
    fn plan(
        reading: Self::Reading<'_>,
        command: &Self::Command<'_>,
    ) -> Result<Self::Output, Self::Error>;
}

/// The output of a plan that cannot refuse: its `Result` is always `Ok`.
///
/// Used at the call sites of the verdict-shaped plans ([`branch::Decide`],
/// [`timeout::Round`], [`timeout::Dispatch`], [`task_admission::Admit`],
/// [`concurrency::Admit`]) — they answer, they never refuse.
pub(crate) fn infallible<T>(planned: Result<T, std::convert::Infallible>) -> T {
    match planned {
        Ok(output) => output,
        // `Infallible` has no inhabitant to reach here with.
        Err(never) => match never {},
    }
}
