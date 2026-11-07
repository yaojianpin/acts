//! `acts.core.users`: ask the business system to resolve a candidate
//! expression into participants.
//!
//! The package is an IRQ act, not an evaluator: its params *are* the
//! [`CandidateExpr`](crate::model::CandidateExpr) document, and the engine
//! sends it to the client exactly like any other request — with the local
//! half (`initiator`, `current_actor`, `previous_approver`, user-valued
//! variables, `${{ … }}` strings) already computed by `prepare_params`. The
//! client knows its user directory — roles, groups, resource owners,
//! relations — and answers with the participants:
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
//! A workflow declares the pool on `assign.candidates: { params, uses }` and
//! the answer lands in the act's variables, where
//! `acts.core.parallel.in: "${{ candidates }}"` (or `ids`) fans the
//! participants out; each item's `assign.owner` reads `value`. The engine
//! resolves the local half of the expression (literal users, `initiator`,
//! `current_actor`, `previous_approver`, and the set operations) itself; the
//! business-data half never leaves the business system.
//!
//! The package has no `execute`: an `ActRunAs::Irq` act waits for its client,
//! so there is nothing for the engine to evaluate.

use crate::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Config, Context,
    ContextVar, Participant, Result, VarType, model::CandidateExpr,
};
use serde_json::{Value as JsonValue, json};

/// The default `uses` of `assign.candidates` — the pool this package computes.
pub(crate) const USERS_PACKAGE: &str = crate::model::USERS_PACKAGE;

#[derive(Debug, Clone)]
pub struct UsersPackage;

impl UsersPackage {
    /// Parse the act's params as a [`CandidateExpr`]. The raw document is the
    /// documented form; `{ "candidates": … }` is accepted so a step can name
    /// the expression the same way `assign.candidates.params` does; a plain
    /// string keeps the legacy string form.
    pub(crate) fn expression(params: &JsonValue) -> Result<CandidateExpr> {
        let invalid = |detail: &str| {
            ActError::Package(format!(
                "invalid ActPackage(acts.core.users) params: {detail}"
            ))
        };
        match params {
            JsonValue::String(_) => {
                serde_json::from_value(params.clone()).map_err(|error| invalid(&format!("{error}")))
            }
            JsonValue::Object(map) => {
                let value = if map.contains_key("kind") {
                    params
                } else if let Some(candidates) = map.get("candidates") {
                    candidates
                } else {
                    return Err(invalid(
                        "expected a candidate expression with a 'kind' or a 'candidates' field",
                    ));
                };
                serde_json::from_value(value.clone()).map_err(|error| invalid(&format!("{error}")))
            }
            JsonValue::Null => Err(invalid("expected a candidate expression")),
            other => Err(invalid(&format!(
                "expected a candidate expression, got {}",
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
impl ActPackage for UsersPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: USERS_PACKAGE,
            name: "Users",
            desc: "send a candidate expression to the business system and receive the participants (id, code, name)",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>"#,
            doc: "",
            schema: json!({
                "type": ["object", "string"],
                "title": "Candidate expression",
                "description": "The candidate expression the business system resolves. Raw form (`{ kind: … }`) or `{ candidates: <expr> }`. The client answers with `participants: [{id, code, name}]`, `ids: [id]` and `count`; the answer lands in the act's variables for block/parallel/sequence fan-out.",
                "examples": [
                    { "kind": "owners_of",
                      "resource": { "resource": "department", "name": "rd-dept" },
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

    fn new(_config: &Config) -> Result<Self> {
        Ok(Self)
    }

    /// Replace the expression's local context atoms — and its `${{ … }}`
    /// strings — with the users they resolved to before the request reaches
    /// the business system.
    ///
    /// The business system owns roles, groups, resources and relations, but
    /// the engine owns the run facts (`initiator`, `current_actor`,
    /// `previous_approver`), user-valued workflow variables and the `${{ … }}`
    /// strings over them. Resolving them here keeps the request
    /// self-contained and leaves only business atoms for the client. The
    /// request always carries the raw expression, whether the act declared it
    /// raw or wrapped as `{ "candidates": … }`.
    async fn prepare_params(&self, ctx: &Context, params: JsonValue) -> Result<JsonValue> {
        let expression = Self::expression(&params)?;
        let expression = resolve_local(ctx, &expression)?;
        Ok(serde_json::to_value(expression)?)
    }
}

/// A [`CandidateExpr`] with its local context atoms replaced.
fn resolve_local(ctx: &Context, expr: &CandidateExpr) -> Result<CandidateExpr> {
    Ok(match expr {
        CandidateExpr::Value(value) => {
            // A `${{ … }}` string is a variable param: the engine computes it
            // locally (the same fill a package param uses), so the request
            // carries the resolved users, never an expression. A literal id
            // becomes itself. An expression the scope cannot answer fails the
            // request: the engine already knows the value is absent, so
            // sending it on would only hide the mistake.
            let resolved = crate::utils::fill_params(&JsonValue::String(value.clone()), ctx);
            if resolved.is_null() && !crate::utils::get_exprs(value).is_empty() {
                return Err(ActError::Package(format!(
                    "candidates expression '{value}' reads a value this scope cannot answer"
                )));
            }
            participants_to_expr(&crate::model::participants_from_json(&resolved)?)
        }
        CandidateExpr::User { .. }
        | CandidateExpr::Role { .. }
        | CandidateExpr::Group { .. }
        | CandidateExpr::OwnersOf { .. } => expr.clone(),
        CandidateExpr::Context { var } => resolve_context(ctx, var)?,
        CandidateExpr::Relate { from, rel, duty } => CandidateExpr::Relate {
            from: Box::new(resolve_local(ctx, from)?),
            rel: rel.clone(),
            duty: duty.clone(),
        },
        CandidateExpr::AnyOf { operands } => CandidateExpr::AnyOf {
            operands: operands
                .iter()
                .map(|operand| resolve_local(ctx, operand))
                .collect::<Result<Vec<_>>>()?,
        },
        CandidateExpr::AllOf { operands } => CandidateExpr::AllOf {
            operands: operands
                .iter()
                .map(|operand| resolve_local(ctx, operand))
                .collect::<Result<Vec<_>>>()?,
        },
        CandidateExpr::Except { base, minus } => CandidateExpr::Except {
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
fn resolve_context(ctx: &Context, var: &ContextVar) -> Result<CandidateExpr> {
    if let ContextVar::Var { ty, .. } = var
        && !matches!(ty, VarType::User | VarType::Users)
    {
        return Ok(CandidateExpr::Context { var: var.clone() });
    }
    let node = CandidateExpr::Context { var: var.clone() };
    // The package's local half is the kernel's deterministic local evaluation
    // (`caps/assign`'s owner evaluation) — never a pluggable resolver, so the
    // package cannot depend on whichever implementation happens to be
    // attached.
    let participants = ctx.resolve_local_owners(&node)?;
    Ok(participants_to_expr(&participants))
}

/// The set expression for resolved participants: no user is an empty set, one
/// is a `user`, several are an `any_of` of users.
fn participants_to_expr(participants: &[Participant]) -> CandidateExpr {
    match participants {
        [] => CandidateExpr::AnyOf {
            operands: Vec::new(),
        },
        [participant] => CandidateExpr::User {
            name: participant.id.clone(),
        },
        participants => CandidateExpr::AnyOf {
            operands: participants
                .iter()
                .map(|participant| CandidateExpr::User {
                    name: participant.id.clone(),
                })
                .collect(),
        },
    }
}

inventory::submit!(crate::package::ActPackageRegister::new::<UsersPackage>());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_package_is_an_irq_request() {
        let meta = UsersPackage::definition();
        assert_eq!(meta.id, "acts.core.users");
        assert_eq!(meta.run_as, ActRunAs::Irq);
        assert_eq!(meta.catalog, ActPackageCatalog::Core);
    }

    #[test]
    fn the_params_parse_as_documented_expressions() {
        for params in [
            json!({ "kind": "role", "name": "reviewer" }),
            json!("alice"),
            json!({ "candidates": { "kind": "role", "name": "reviewer" } }),
        ] {
            assert!(UsersPackage::expression(&params).is_ok(), "{params}");
        }
        for params in [JsonValue::Null, json!({ "name": "x" }), json!(3)] {
            assert!(UsersPackage::expression(&params).is_err(), "{params}");
        }
    }

    #[test]
    fn answers_parse_into_participants() {
        let answer = |value: &JsonValue| crate::model::participants_from_json(value);
        assert!(answer(&JsonValue::Null).unwrap().is_empty());
        let one = answer(&json!("u1")).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].id, "u1");
        let many = answer(&json!([{ "id": "u1" }, "u2"])).unwrap();
        assert_eq!(many.len(), 2);
    }
}
