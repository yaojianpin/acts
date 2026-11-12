//! Evaluation of candidate expressions.
//!
//! [`evaluate`] resolves a [`CandidateExpr`]'s local half — literal users,
//! the context variables (`initiator`, `current_actor`, `previous_approver`),
//! variables holding users, and the set operations. Business-data atoms
//! (roles, groups, resource owners and relations) are deliberately not
//! resolved here: an `assign.candidates` spec sends the expression to its
//! `uses` package (`acts.core.users` by default) and the business
//! system's answer carries the participants. A local evaluation that meets
//! one of those atoms fails — the start treats the failure as "not locally
//! computable" and asks the package instead.
//!
//! [`evaluate_owner`] is the one-`string` form an `assign.owner` declares: a
//! literal user id or a `${{ … }}` expression, evaluated on the same
//! environment a package param uses. This is the kernel's own implementation
//! of the owner-resolution seam
//! ([`AssignmentResolver`](crate::scheduler::shell::caps::assign::AssignmentResolver)); a
//! serving assembly may install its own.

use crate::scheduler::shell::Context;
use crate::{
    ActError, Result,
    model::{CandidateExpr, ContextVar, Participant, VarType, participants_from_json},
};
use serde_json::Value as JsonValue;
use std::collections::HashSet;

/// Whether the expression names business data the kernel has no directory
/// for: roles, groups, resource owners, relations, or a business-typed
/// variable. Such an expression's pool computes through the spec's `uses`
/// package; an expression without one computes locally.
pub(crate) fn names_business_data(expr: &CandidateExpr) -> bool {
    match expr {
        CandidateExpr::Value(_) | CandidateExpr::User { .. } => false,
        CandidateExpr::Role { .. }
        | CandidateExpr::Group { .. }
        | CandidateExpr::OwnersOf { .. }
        | CandidateExpr::Relate { .. } => true,
        CandidateExpr::Context {
            var: ContextVar::Var { ty, .. },
        } => matches!(
            ty,
            VarType::Role | VarType::Group | VarType::Project | VarType::Department | VarType::Unit
        ),
        CandidateExpr::Context { .. } => false,
        CandidateExpr::AnyOf { operands } | CandidateExpr::AllOf { operands } => {
            operands.iter().any(names_business_data)
        }
        CandidateExpr::Except { base, minus } => {
            names_business_data(base) || names_business_data(minus)
        }
    }
}

/// Evaluate the owner string an act declares: a literal user id or a
/// `${{ … }}` expression. An unresolved value is no participants (the assign
/// caller fails the act rather than inventing an owner).
pub(crate) fn evaluate_owner(ctx: &Context, owner: &str) -> Result<Vec<Participant>> {
    let resolved = crate::utils::fill_params(&JsonValue::String(owner.to_string()), ctx);
    checked(participants_from_json(&resolved)?)
}

/// Evaluate `expr` against the acting scope.
///
/// Only local atoms resolve. The start's structural check
/// ([`names_business_data`]) routes an expression naming business data to the
/// spec's `uses` package before this runs; the business arms here stay as the
/// evaluation's own refusal.
pub(crate) fn evaluate(ctx: &Context, expr: &CandidateExpr) -> Result<Vec<Participant>> {
    match expr {
        CandidateExpr::Value(value) => {
            // A literal user id, or a `${{ … }}` expression the kernel
            // computes locally (the same fill a package param uses). An
            // expression this scope cannot answer fails the act's start: the
            // engine already knows the value is absent, so it invents neither
            // an empty pool nor a request.
            let resolved = crate::utils::fill_params(&JsonValue::String(value.clone()), ctx);
            if resolved.is_null() && !crate::utils::get_exprs(value).is_empty() {
                return Err(ActError::Action(format!(
                    "candidates expression '{value}' reads a value this scope cannot answer"
                )));
            }
            checked(participants_from_json(&resolved)?)
        }
        CandidateExpr::User { value: name } => {
            if name.trim().is_empty() {
                return Ok(Vec::new());
            }
            Ok(vec![Participant::new(name)])
        }
        CandidateExpr::Context { var } => checked(context_participants(ctx, var)?),
        CandidateExpr::AnyOf { operands } => {
            let mut participants = Vec::new();
            for operand in operands {
                participants.extend(evaluate(ctx, operand)?);
            }
            checked(participants)
        }
        CandidateExpr::AllOf { operands } => {
            let mut sets = Vec::with_capacity(operands.len());
            for operand in operands {
                sets.push(checked(evaluate(ctx, operand)?)?);
            }
            let Some(first) = sets.first() else {
                return Ok(Vec::new());
            };
            let mut selected = first.clone();
            for other in &sets[1..] {
                let ids: HashSet<&str> = other
                    .iter()
                    .map(|participant| participant.id.as_str())
                    .collect();
                selected.retain(|participant| ids.contains(participant.id.as_str()));
            }
            checked(selected)
        }
        CandidateExpr::Except { base, minus } => {
            let mut base = evaluate(ctx, base)?;
            let minus = evaluate(ctx, minus)?;
            let minus: HashSet<&str> = minus
                .iter()
                .map(|participant| participant.id.as_str())
                .collect();
            base.retain(|participant| !minus.contains(participant.id.as_str()));
            checked(base)
        }
        CandidateExpr::Role { name } => Err(business_atom("role", name)),
        CandidateExpr::Group { name } => Err(business_atom("group", name)),
        CandidateExpr::OwnersOf { resource, .. } => {
            Err(business_atom(resource.kind(), resource.name()))
        }
        CandidateExpr::Relate { rel, .. } => Err(business_atom("relation", rel.name())),
    }
}

/// Resolve one runtime context variable. Context atoms answer the acting
/// scope; a `var` atom reads the workflow variable through the same
/// expression environment a package sees. A variable that names roles,
/// groups or resources is business data and is refused here.
fn context_participants(ctx: &Context, var: &ContextVar) -> Result<Vec<Participant>> {
    match var {
        ContextVar::Initiator => Ok(ctx
            .initiator()
            .map(|id| vec![Participant::new(&id)])
            .unwrap_or_default()),
        ContextVar::CurrentActor => Ok(ctx
            .current_actor()
            .map(|id| vec![Participant::new(&id)])
            .unwrap_or_default()),
        ContextVar::PreviousApprover => Ok(ctx
            .previous_approver(0)
            .map(|id| vec![Participant::new(&id)])
            .unwrap_or_default()),
        ContextVar::PreviousApproverNth { back } => Ok(ctx
            .previous_approver(*back)
            .map(|id| vec![Participant::new(&id)])
            .unwrap_or_default()),
        ContextVar::Var { name, ty, duty } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(ActError::Action(
                    "owner expression reads an unnamed context variable".to_string(),
                ));
            }
            match ty {
                VarType::User | VarType::Users => {
                    let value = Context::scope(ctx, || ctx.runtime.env().eval::<JsonValue>(name))?;
                    participants_from_json(&value)
                }
                VarType::Role | VarType::Group => Err(business_variable(*ty, name, duty)),
                VarType::Project | VarType::Department | VarType::Unit => {
                    Err(business_variable(*ty, name, duty))
                }
            }
        }
    }
}

fn business_atom(kind: &str, name: &str) -> ActError {
    ActError::Action(format!(
        "candidates atom '{kind}' ('{name}') names business data; the pool computes through the spec's uses package (acts.core.users by default), which passes this expression to the business system and its answer carries the participants"
    ))
}

fn business_variable(ty: VarType, name: &str, duty: &crate::model::Duty) -> ActError {
    ActError::Action(format!(
        "candidates variable '{name}' is typed {ty:?} under duty {duty:?}, which names business data; the pool computes through the spec's uses package (acts.core.users by default)"
    ))
}

/// Reject participants without an id (a value must not hand out an
/// unaddressable user) and drop duplicates while preserving order.
fn checked(participants: Vec<Participant>) -> Result<Vec<Participant>> {
    let mut seen = HashSet::with_capacity(participants.len());
    let mut result = Vec::with_capacity(participants.len());
    for participant in participants {
        if !participant.is_valid() {
            return Err(ActError::Action(
                "owner expression resolved to a participant without an id".to_string(),
            ));
        }
        if seen.insert(participant.id.clone()) {
            result.push(participant);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::model::{CandidateExpr, Participant};
    use serde_json::json;

    /// The set operations are pure participant algebra.
    #[test]
    fn a_participant_list_is_deduplicated_by_id() {
        let participants = vec![
            Participant::with_names("u1", "a", "Alice"),
            Participant::with_names("u1", "a", "Alice again"),
            Participant::with_names("u2", "b", "Bob"),
        ];
        let checked = super::checked(participants).unwrap();
        assert_eq!(checked.len(), 2);
        assert_eq!(checked[0].name, "Alice");
        assert_eq!(checked[1].id, "u2");
    }

    #[test]
    fn an_owner_document_deserializes_into_the_legacy_value_variant() {
        let expr: CandidateExpr = serde_json::from_value(json!("alice")).unwrap();
        assert!(matches!(expr, CandidateExpr::Value(value) if value == "alice"));
    }

    #[test]
    fn a_business_atom_names_the_irq_package() {
        let error = super::business_atom("role", "reviewer");
        assert!(error.to_string().contains("acts.core.users"));
    }
}
