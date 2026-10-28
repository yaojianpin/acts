//! `acts.core.owner`: ask the business system to resolve an owner expression
//! into participants.
//!
//! The package is an IRQ act, not an evaluator: its params *are* the
//! [`OwnerExpr`](crate::OwnerExpr) document (raw, or wrapped as
//! `{ "owner": … }`), and the engine sends it to the client exactly like any
//! other request. The client knows its user directory — roles, groups,
//! resource owners, relations — and answers with the participants:
//!
//! ```json
//! {
//!   "participants": [
//!     { "id": "u1", "code": "alice", "name": "Alice" },
//!     { "id": "u2", "code": "bob", "name": "Bob" }
//!   ],
//!   "ids": ["u1", "u2"],
//!   "count": 2
//! }
//! ```
//!
//! A workflow then fans the participants out through
//! `acts.core.parallel.in: "${{ owners.participants }}"` (or `ids`), and each
//! item's `assign.owner` reads `value.id`. The engine resolves the local half
//! of the expression (literal users, `initiator`, `current_actor`,
//! `previous_approver`, and the set operations) itself; the business-data half
//! never leaves the business system.
//!
//! The package has no `execute`: an `ActRunAs::Irq` act waits for its client,
//! so there is nothing for the engine to evaluate.

use crate::{
    Context, OwnerExpr, Participant,
    model::{ContextVar, VarType},
    package::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, ActRunAs},
};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone)]
pub struct OwnerPackage;

impl OwnerPackage {
    /// Parse the act's params as an [`OwnerExpr`]. The raw document is the
    /// documented form; `{ "owner": … }` is accepted so a step can name the
    /// expression the same way `assign.owner` does; a plain string keeps the
    /// legacy string form.
    fn expression(params: &JsonValue) -> crate::Result<OwnerExpr> {
        let invalid = |detail: &str| {
            crate::ActError::Package(format!(
                "invalid ActPackage(acts.core.owner) params: {detail}"
            ))
        };
        match params {
            JsonValue::String(_) => {
                serde_json::from_value(params.clone()).map_err(|error| invalid(&format!("{error}")))
            }
            JsonValue::Object(map) => {
                let value = if map.contains_key("kind") {
                    params
                } else if let Some(owner) = map.get("owner") {
                    owner
                } else {
                    return Err(invalid(
                        "expected an owner expression with a 'kind' or an 'owner' field",
                    ));
                };
                serde_json::from_value(value.clone()).map_err(|error| invalid(&format!("{error}")))
            }
            JsonValue::Null => Err(invalid("expected an owner expression")),
            other => Err(invalid(&format!(
                "expected an owner expression, got {}",
                match other {
                    JsonValue::Bool(_) => "a boolean",
                    JsonValue::Number(_) => "a number",
                    JsonValue::Array(_) => "an array",
                    _ => "a value",
                }
            ))),
        }
    }
}

#[async_trait::async_trait]
impl ActPackage for OwnerPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.owner",
            name: "Owner participants",
            desc: "send an owner expression to the business system and receive the participants (id, code, name)",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>"#,
            doc: "",
            schema: json!({
                "type": ["object", "string"],
                "title": "Owner expression",
                "description": "The owner expression the business system resolves. Raw form (`{ kind: … }`) or `{ owner: <expr> }`. The client answers with `participants: [{id, code, name}]`, `ids: [id]` and `count`; use `participants` with acts.core.parallel.in and `value.id` in the item's assign.owner.",
                "examples": [
                    { "kind": "owners_of",
                      "resource": { "resource": "department", "name": "研发部" },
                      "duty": "principal" },
                    { "kind": "relate",
                      "from": { "kind": "context", "var": { "type": "current_actor" } },
                      "rel": { "rel": "resource", "value": "department_of" },
                      "duty": "principal" },
                    { "kind": "any_of", "operands": [
                        { "kind": "context", "var": { "type": "initiator" } },
                        { "kind": "role", "name": "reviewer" }
                    ]}
                ]
            }),
            options: Some(json!({
                "ui:order": ["kind", "operands", "from", "rel", "resource", "var", "name", "duty"],
            })),
            run_as: ActRunAs::Irq,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }

    fn new(_config: &crate::Config) -> crate::Result<Self> {
        Ok(Self)
    }

    /// Replace the expression's local context atoms with the users they
    /// resolved to before the request reaches the business system.
    ///
    /// The business system owns roles, groups, resources and relations, but
    /// the engine owns the run facts (`initiator`, `current_actor`,
    /// `previous_approver`) and user-valued workflow variables. Resolving them
    /// here keeps the request self-contained and leaves only business atoms
    /// for the client. The request always carries the raw expression, whether
    /// the act declared it raw or wrapped as `{ "owner": … }`.
    async fn prepare_params(&self, ctx: &Context, params: JsonValue) -> crate::Result<JsonValue> {
        let expression = Self::expression(&params)?;
        let expression = resolve_local(ctx, &expression)?;
        Ok(serde_json::to_value(expression)?)
    }
}

/// An [`OwnerExpr`] with its local context atoms replaced.
fn resolve_local(ctx: &Context, expr: &OwnerExpr) -> crate::Result<OwnerExpr> {
    Ok(match expr {
        OwnerExpr::Value(_)
        | OwnerExpr::User { .. }
        | OwnerExpr::Role { .. }
        | OwnerExpr::Group { .. }
        | OwnerExpr::OwnersOf { .. } => expr.clone(),
        OwnerExpr::Context { var } => resolve_context(ctx, var)?,
        OwnerExpr::Relate { from, rel, duty } => OwnerExpr::Relate {
            from: Box::new(resolve_local(ctx, from)?),
            rel: rel.clone(),
            duty: duty.clone(),
        },
        OwnerExpr::AnyOf { operands } => OwnerExpr::AnyOf {
            operands: operands
                .iter()
                .map(|operand| resolve_local(ctx, operand))
                .collect::<crate::Result<Vec<_>>>()?,
        },
        OwnerExpr::AllOf { operands } => OwnerExpr::AllOf {
            operands: operands
                .iter()
                .map(|operand| resolve_local(ctx, operand))
                .collect::<crate::Result<Vec<_>>>()?,
        },
        OwnerExpr::Except { base, minus } => OwnerExpr::Except {
            base: Box::new(resolve_local(ctx, base)?),
            minus: Box::new(resolve_local(ctx, minus)?),
        },
    })
}

/// Resolve one context atom. A business-typed variable (`role`, `group`,
/// `project`, `department`, `unit`) is left for the business system; a local
/// atom becomes the users it resolved to. A local atom the scope cannot
/// answer (an unknown variable) fails the request: the engine already knows
/// the value is absent, so sending it on would only hide the mistake.
fn resolve_context(ctx: &Context, var: &ContextVar) -> crate::Result<OwnerExpr> {
    if let ContextVar::Var { ty, .. } = var
        && !matches!(ty, VarType::User | VarType::Users)
    {
        return Ok(OwnerExpr::Context { var: var.clone() });
    }
    let node = OwnerExpr::Context { var: var.clone() };
    let participants = ctx.resolve_owners(&node)?;
    Ok(participants_to_expr(&participants))
}

/// The set expression for resolved participants: no user is an empty set, one
/// is a `user`, several are an `any_of` of users.
fn participants_to_expr(participants: &[Participant]) -> OwnerExpr {
    match participants {
        [] => OwnerExpr::AnyOf {
            operands: Vec::new(),
        },
        [participant] => OwnerExpr::User {
            name: participant.id.clone(),
        },
        participants => OwnerExpr::AnyOf {
            operands: participants
                .iter()
                .map(|participant| OwnerExpr::User {
                    name: participant.id.clone(),
                })
                .collect(),
        },
    }
}

inventory::submit!(ActPackageRegister::new::<OwnerPackage>());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActPackage;

    #[test]
    fn the_owner_package_is_an_irq_request() {
        let meta = OwnerPackage::definition();
        assert_eq!(meta.id, "acts.core.owner");
        assert_eq!(meta.run_as, ActRunAs::Irq);
        assert_eq!(meta.catalog, ActPackageCatalog::Core);
    }

    #[test]
    fn the_owner_package_schema_accepts_the_documented_forms() {
        let meta = OwnerPackage::definition();
        for params in [
            json!({ "kind": "context", "var": { "type": "current_actor" } }),
            json!({ "kind": "owners_of",
                    "resource": { "resource": "department", "name": "研发部" },
                    "duty": "principal" }),
            json!({ "owner": { "kind": "role", "name": "reviewer" } }),
            json!("${{ reviewer }}"),
        ] {
            jsonschema::validate(&meta.schema, &params).unwrap();
        }
    }
}
