//! The owner-resolution seam: how an act's declared `assign.owner` string
//! becomes its one owner at start.
//!
//! The contract is the same headless scope spec a package's params resolve
//! through plus the declared owner (a literal user id or a `${{ … }}`
//! expression); the answer is at most one participant id. The kernel's own
//! implementation (`scheduler::shell::RuntimeAssignmentResolver`) evaluates
//! the string on the engine's environment and fails the act rather than leave
//! it unowned; a serving assembly may install its own before driving
//! operations (see [`crate::scheduler::shell::caps::services::CapServices`]).

use super::Capability;

/// The `assign.owner` resolution seam (see [`Capability`]).
#[async_trait::async_trait]
pub(crate) trait AssignmentResolver: Capability {
    /// The single owner the string resolves to; `None` for an empty
    /// resolution (the act must fail rather than run unowned). A resolution
    /// with more than one participant is an error: a human act has one owner.
    async fn resolve_owner(
        &self,
        scope: crate::scheduler::ScopeSpec,
        owner: &str,
    ) -> crate::Result<Option<String>>;

    /// The local computation of one `assign.candidates` expression on the
    /// acting scope. `Err` means "not locally computable" — a business-data
    /// atom the implementation has no directory for, or a local atom the
    /// scope cannot answer — and the candidates orchestration asks the
    /// spec's `uses` package instead.
    async fn compute_pool(
        &self,
        scope: crate::scheduler::ScopeSpec,
        expr: &crate::model::CandidateExpr,
    ) -> crate::Result<Vec<crate::model::Participant>>;
}
