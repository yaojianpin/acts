//! Evaluation of an [`OwnerExpr`](crate::model::OwnerExpr).
//!
//! The engine resolves the local half — literal users, the context variables
//! (`initiator`, `current_actor`, `previous_approver`), variables holding
//! users, and the set operations. Business-data atoms (roles, groups, resource
//! owners and relations) are deliberately not resolved here: they are sent to
//! the business system by the `acts.core.owner` IRQ package, whose params are
//! the same expression and whose answer carries the participants. An
//! expression that names one of those atoms outside that package fails with a
//! message that says so, instead of inventing an owner.

use super::Context;
use crate::{
    ActError, Result,
    model::{ContextVar, OwnerExpr, Participant, VarType, participants_from_json},
};
use serde_json::Value as JsonValue;
use std::collections::HashSet;

/// Evaluate `expr` against the acting scope.
///
/// Only local atoms resolve; a business-data atom is refused with guidance to
/// the `acts.core.owner` IRQ package.
pub(crate) fn evaluate(ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>> {
    match expr {
        OwnerExpr::Value(value) => {
            // The legacy form: a literal user id or an expression. The
            // expression path is the same one a package param uses; an
            // unresolved value is no participants (the assign caller fails
            // the act rather than inventing one).
            let resolved = crate::utils::fill_params(&JsonValue::String(value.clone()), ctx);
            checked(participants_from_json(&resolved)?)
        }
        OwnerExpr::User { name } => {
            if name.trim().is_empty() {
                return Ok(Vec::new());
            }
            Ok(vec![Participant::new(name)])
        }
        OwnerExpr::Context { var } => checked(context_participants(ctx, var)?),
        OwnerExpr::AnyOf { operands } => {
            let mut participants = Vec::new();
            for operand in operands {
                participants.extend(evaluate(ctx, operand)?);
            }
            checked(participants)
        }
        OwnerExpr::AllOf { operands } => {
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
        OwnerExpr::Except { base, minus } => {
            let mut base = evaluate(ctx, base)?;
            let minus = evaluate(ctx, minus)?;
            let minus: HashSet<&str> = minus
                .iter()
                .map(|participant| participant.id.as_str())
                .collect();
            base.retain(|participant| !minus.contains(participant.id.as_str()));
            checked(base)
        }
        OwnerExpr::Role { name } => Err(business_atom("role", name)),
        OwnerExpr::Group { name } => Err(business_atom("group", name)),
        OwnerExpr::OwnersOf { resource, .. } => {
            Err(business_atom(resource.kind(), resource.name()))
        }
        OwnerExpr::Relate { rel, .. } => Err(business_atom("relation", rel.name())),
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
        "owner atom '{kind}' ('{name}') names business data; resolve it through the acts.core.owner IRQ package — pass this expression as the package's params, let the business system return the participants, and assign from the returned ids"
    ))
}

fn business_variable(ty: VarType, name: &str, duty: &crate::model::Duty) -> ActError {
    ActError::Action(format!(
        "owner variable '{name}' is typed {ty:?} under duty {duty:?}, which names business data; resolve it through the acts.core.owner IRQ package — pass this expression as the package's params and assign from the returned ids"
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
    use crate::model::{OwnerExpr, Participant};
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
        let expr: OwnerExpr = serde_json::from_value(json!("alice")).unwrap();
        assert!(matches!(expr, OwnerExpr::Value(value) if value == "alice"));
    }

    #[test]
    fn a_business_atom_names_the_irq_package() {
        let error = super::business_atom("role", "reviewer");
        assert!(error.to_string().contains("acts.core.owner"));
    }
}
