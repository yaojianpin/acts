use crate::JoinRule;
use crate::ValidationRule;
use crate::Variant;
use crate::{ModelBase, Vars, model::Branch, model::CandidateExpr};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

/// The declared assignment of one human decision act: the expression the act
/// resolves at start into its persisted `__assignment` task state, and — for
/// multi-approver steps — the candidate pool its `uses` package computes.
/// Only a human decision leaf (`ActRunAs::Irq`) carries its own `owner`; a
/// container step (`block`/`parallel`/`sequence`) may declare an `owner` as
/// the template its generated children inherit (the container itself never
/// holds an assignment) and `candidates` (or any named key) for the fan-out.
/// The kernel refuses an `assign` on a subflow or a `Func`/`Msg` leaf when
/// the act starts.
///
/// The three assignment vocabularies are separate capabilities (see
/// `crate::scheduler::shell::caps::assign`):
///
/// - `owner` — the one responsible person (`assignee`); the act's decisions
///   must come from them.
/// - `candidates` — the list of people the container fans acts out over; each
///   generated child carries its own owner.
/// - any other key — a named pool processed exactly like `candidates`: its
///   computed answer lands under its own key (`$assign.<key>.users`,
///   `$assign.<key>.ids`), next to `$assign.candidates.users` /
///   `$assign.candidates.ids`. The declaration value is the bare expression
///   (`cc: "abc"`) or the full spec (`my_name: { params, uses }`).
/// - delegation — runtime state (`delegator`/`delegatee`), moved with
///   `act:delegate`/`act:undelegate`, never declared in the model.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Assign {
    /// The owner: a literal user id, a `${{ … }}` expression, or absent. The
    /// resolution must contain exactly one participant — a human act has one
    /// owner; an empty or larger resolution fails the act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The candidate pool: a `uses` package computes it from `params` at act
    /// start and the answer lands in the act's variables, where the
    /// container's fan-out (`block`/`parallel`/`sequence`) reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Box<Candidates>>,
    /// Every other key of the declaration, each a named pool processed
    /// exactly like `candidates` (see [`Assign`]). `owner` and `candidates`
    /// are the named fields and never appear here; the runtime refuses the
    /// engine-reserved names on the rest.
    ///
    /// `flatten` shapes the serialize side only — the declarations spread
    /// next to `owner`/`candidates`. The deserialize side is the hand-written
    /// map walk below, because flatten buffers every value into private
    /// content and a buffered value carries no YAML position.
    #[serde(flatten)]
    pub others: std::collections::BTreeMap<String, Candidates>,
}

impl<'de> Deserialize<'de> for Assign {
    /// A hand-written map walk instead of the derive plus `#[serde(flatten)]`:
    /// flatten buffers the whole map into `serde::de::Content` before the
    /// named-pool values ever parse, and a buffered parse carries no YAML
    /// position — a broken pool would report the start of the whole `assign`
    /// map (`data did not match any variant of untagged enum Form`) instead
    /// of the exact line that failed. Reading every value straight off the
    /// live deserializer keeps [`Candidates`]'s errors on their own line.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct AssignVisitor;

        impl<'de> Visitor<'de> for AssignVisitor {
            type Value = Assign;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an assignment declaration: an owner, a pool or named pools")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut assign = Assign::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "owner" => assign.owner = map.next_value()?,
                        "candidates" => assign.candidates = map.next_value()?,
                        _ => {
                            assign.others.insert(key, map.next_value()?);
                        }
                    }
                }
                Ok(assign)
            }
        }

        deserializer.deserialize_map(AssignVisitor)
    }
}

/// One act's candidate-pool declaration (see [`Assign::candidates`]).
///
/// `uses` names the IRQ package that computes the pool — the core
/// [`USERS_PACKAGE`] by default, or any package answering the same
/// contract (`{ participants, ids, count }`). `params` is the package's
/// input: a [`CandidateExpr`] the engine computes locally when every atom
/// resolves in the kernel, and otherwise sends to the package with its
/// local context atoms already resolved.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Candidates {
    /// The computing package's params.
    #[serde(default)]
    pub params: CandidateExpr,
    /// The package that computes the pool. Defaults to
    /// [`USERS_PACKAGE`]; a model names its own extension here.
    #[serde(default = "default_users_uses")]
    pub uses: String,
}

impl<'de> Deserialize<'de> for Candidates {
    /// Accepts the full spec (`{ params, uses }`) and, like the expression
    /// itself, its bare forms: a string (`cc: "abc"`) or a `kind`-tagged
    /// object (`my_name: { kind: context, var: … }`) — both become `params`
    /// under the default `uses`.
    ///
    /// The shape is decided from the first key — `kind` starts a bare
    /// expression, `params`/`uses` a spec, anything else a declaration whose
    /// keys are ordered otherwise and must be buffered before the two shapes
    /// are told apart — instead of an untagged enum: an untagged enum buffers
    /// the value before it tries the variants, and a buffered value carries
    /// no YAML position, so every failure reported as `data did not match
    /// any variant of untagged enum Form` at the value's start. Parsed this
    /// way, the common shapes read straight off the live deserializer and
    /// their errors keep the exact line and column.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CandidatesVisitor;

        impl<'de> Visitor<'de> for CandidatesVisitor {
            type Value = Candidates;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a candidate expression or a { params, uses } spec")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(Candidates {
                    params: CandidateExpr::Value(value.to_string()),
                    uses: default_users_uses(),
                })
            }

            fn visit_string<E: serde::de::Error>(
                self,
                value: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(Candidates {
                    params: CandidateExpr::Value(value),
                    uses: default_users_uses(),
                })
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let Some(first) = map.next_key::<String>()? else {
                    // An empty declaration is an empty spec: default params
                    // and the default users package.
                    return Ok(Candidates::default());
                };
                match first.as_str() {
                    // A bare expression: the tag opens the object, its word
                    // picks the payload, and the remaining entries are the
                    // payload's fields — live, so a bad field reports its
                    // own line.
                    "kind" => {
                        let kind: String = map.next_value()?;
                        let params = crate::model::owner::dispatch_expression(
                            &kind,
                            serde::de::value::MapAccessDeserializer::new(map),
                        )?;
                        Ok(Candidates {
                            params,
                            uses: default_users_uses(),
                        })
                    }
                    // A spec: `params` and `uses` parse live; anything else
                    // is not this declaration's business and is ignored,
                    // exactly as the previous spec shape did.
                    "params" | "uses" => {
                        let mut params: Option<CandidateExpr> = None;
                        let mut uses: Option<String> = None;
                        match first.as_str() {
                            "params" => params = Some(map.next_value()?),
                            _ => uses = Some(map.next_value()?),
                        }
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                "params" => params = Some(map.next_value()?),
                                "uses" => uses = Some(map.next_value()?),
                                _ => {
                                    map.next_value::<IgnoredAny>()?;
                                }
                            }
                        }
                        Ok(Candidates {
                            params: params.unwrap_or_default(),
                            uses: uses.unwrap_or_else(default_users_uses),
                        })
                    }
                    // Neither shape opens the object: the declaration's keys
                    // are ordered otherwise (a value map built sorted, for
                    // example). Buffer the object and let the tag decide
                    // between the two shapes; buffered values carry no parse
                    // position anyway.
                    other => {
                        let mut object = serde_json::Map::new();
                        object.insert(other.to_string(), map.next_value::<serde_json::Value>()?);
                        while let Some(key) = map.next_key::<String>()? {
                            object.insert(key, map.next_value::<serde_json::Value>()?);
                        }
                        if let Some(kind) = object.remove("kind") {
                            let serde_json::Value::String(kind) = kind else {
                                return Err(serde::de::Error::custom(
                                    "the 'kind' of a candidate expression must be a word",
                                ));
                            };
                            let params = crate::model::owner::dispatch_expression(
                                &kind,
                                serde_json::Value::Object(object),
                            )
                            .map_err(serde::de::Error::custom)?;
                            Ok(Candidates {
                                params,
                                uses: default_users_uses(),
                            })
                        } else {
                            let SpecExpr { params, uses } =
                                SpecExpr::deserialize(serde_json::Value::Object(object))
                                    .map_err(serde::de::Error::custom)?;
                            Ok(Candidates { params, uses })
                        }
                    }
                }
            }
        }

        deserializer.deserialize_any(CandidatesVisitor)
    }
}

/// The `{ params, uses }` spec shape, for declarations that arrive with the
/// keys ordered otherwise than by this module and had to be buffered.
#[derive(Deserialize)]
struct SpecExpr {
    #[serde(default)]
    params: CandidateExpr,
    #[serde(default = "default_users_uses")]
    uses: String,
}

impl Default for Candidates {
    fn default() -> Self {
        Self {
            params: CandidateExpr::default(),
            uses: default_users_uses(),
        }
    }
}

/// The core package that computes a candidate pool — `assign.candidates`'s
/// default `uses`. A model may name its own computing package instead; the
/// kernel only defines the request (the prepared `params`) and the answer
/// contract (`{ participants, ids, count }`).
pub const USERS_PACKAGE: &str = "acts.core.users";

fn default_users_uses() -> String {
    USERS_PACKAGE.to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub id: String,

    /// define the step vars
    #[serde(default)]
    pub vars: Vec<Variant>,

    #[serde(default)]
    pub r#if: Option<String>,

    /// while condition: re-execute this step while the condition holds
    #[serde(default)]
    pub r#while: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    /// The join rule of this step's parallel container: how many of the
    /// branches (or the `acts.core.parallel`/`acts.core.block{ mode: parallel
    /// }` acts it fans out) must complete before the step continues. Absent
    /// (and `all`) means every branch must complete, the engine's original
    /// shape.
    #[serde(default)]
    pub join: Option<JoinRule>,

    // to use a package, such as 'acts.transform.set'
    #[serde(default)]
    pub uses: Option<String>,

    // package params
    #[serde(default)]
    pub params: JsonValue,

    /// The human act's owner declaration. Only a human decision leaf may
    /// carry one; see [`Assign`].
    #[serde(default)]
    pub assign: Option<Assign>,

    #[serde(default)]
    pub catches: Vec<Step>,

    #[serde(default)]
    pub timeouts: Vec<Step>,

    /// extra options to send to client
    #[serde(default)]
    pub options: Vars,

    /// Custom input rules: conditions over the step's scope and its resolved
    /// params, decided when the act starts. The first condition that holds
    /// runs its action (`error` by default — the step fails with
    /// `validation_failed` and the rule's message before the act ever reaches
    /// a client).
    #[serde(default)]
    pub rules: Vec<ValidationRule>,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,

    /// variables to expose
    #[serde(default)]
    pub exposes: Vec<Variant>,

    /// Announce this step's act `Created` message at start even while a
    /// declared candidate pool is still pending. The default defers the
    /// announcement until the pool resolves (the fan-out's resolved facts
    /// only exist then); a step that announces early speaks with its raw
    /// declared params, and its `tid` is a client's handle for step-level
    /// actions (an added-approver push) from the first moment.
    #[serde(default)]
    pub announce: bool,

    /// The kernel's own template step for a container package's `in` fan-out
    /// (`<step>#item`), set by the import
    /// ([`crate::scheduler::shell::model::materialize_container_acts`]) and
    /// never by a declaration. It groups the per-item acts the run fans out;
    /// it is not a declared node, so it speaks no message of its own.
    #[serde(default)]
    pub r#virtual: bool,
}

impl ModelBase for Step {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Step {
    /// Whether this step or any nested step declares an assignment (an
    /// `owner` or a `candidates` pool).
    pub fn has_assignment(&self) -> bool {
        self.assign.is_some()
            || self
                .branches
                .iter()
                .any(|branch| branch.steps.iter().any(Step::has_assignment))
            || self.catches.iter().any(Step::has_assignment)
            || self.timeouts.iter().any(Step::has_assignment)
    }
}

impl Step {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_desc(mut self, desc: &str) -> Self {
        self.desc = desc.to_string();
        self
    }

    pub fn with_uses(mut self, uses: &str, params: Vars) -> Self {
        self.uses = Some(uses.to_string());
        self.params = params.into();
        self
    }

    pub fn with_uses_code(mut self, uses: &str, code: &str) -> Self {
        self.uses = Some(uses.to_string());
        self.params = code.into();
        self
    }

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    pub fn with_if(mut self, r#if: &str) -> Self {
        self.r#if = Some(r#if.to_string());
        self
    }

    pub fn with_while(mut self, r#while: &str) -> Self {
        self.r#while = Some(r#while.to_string());
        self
    }

    /// Set the step's join rule (see [`Step::join`]).
    pub fn with_join(mut self, rule: JoinRule) -> Self {
        self.join = Some(rule);
        self
    }

    /// Add one input validation (see [`Step::rules`]).
    pub fn with_rule(mut self, rule: ValidationRule) -> Self {
        self.rules.push(rule);
        self
    }

    pub fn with_var<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.vars.push(Variant::create(name, json!(value)));
        self
    }

    pub fn vars(&self) -> Vars {
        let mut ret = Vars::new();
        self.vars.iter().for_each(|var| {
            ret.set(&var.name, var.value.clone());
        });

        ret
    }

    pub fn with_option<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.options.set(name, value);
        self
    }

    /// Declare the act's owner (see [`Assign`]): a literal user id or a
    /// `${{ … }}` expression.
    pub fn with_owner(mut self, owner: &str) -> Self {
        self.assign = Some(Assign {
            owner: Some(owner.to_string()),
            candidates: None,
            others: std::collections::BTreeMap::new(),
        });
        self
    }

    /// Declare the act's candidate pool, computed by the core
    /// [`USERS_PACKAGE`] from the expression (see [`Candidates`]).
    pub fn with_candidates(self, params: CandidateExpr) -> Self {
        self.with_candidates_package(USERS_PACKAGE, params)
    }

    /// Declare the act's candidate pool with an explicit computing package:
    /// `uses` names it, `params` is its input expression (see [`Candidates`]).
    pub fn with_candidates_package(mut self, uses: &str, params: CandidateExpr) -> Self {
        let mut assign = self.assign.take().unwrap_or_default();
        assign.candidates = Some(Box::new(Candidates {
            params,
            uses: uses.to_string(),
        }));
        self.assign = Some(assign);
        self
    }

    /// Declare one named pool (see [`Assign::others`]): processed exactly
    /// like `candidates`, with its answer landing under its own key
    /// (`$assign.<name>.users` / `.ids`).
    pub fn with_assign(self, name: &str, params: CandidateExpr) -> Self {
        self.with_assign_package(name, USERS_PACKAGE, params)
    }

    /// Declare one named pool with an explicit computing package (see
    /// [`Step::with_assign`]).
    pub fn with_assign_package(mut self, name: &str, uses: &str, params: CandidateExpr) -> Self {
        let mut assign = self.assign.take().unwrap_or_default();
        assign.others.insert(
            name.to_string(),
            Candidates {
                params,
                uses: uses.to_string(),
            },
        );
        self.assign = Some(assign);
        self
    }

    pub fn with_expose(mut self, var: Variant) -> Self {
        self.exposes.push(var);
        self
    }

    /// Declare `announce`: the step's act `Created` message rides the
    /// candidate-pool wait instead of deferring to the pool's answer.
    pub fn with_announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }

    pub fn exposes(&self) -> Vars {
        let mut vars = Vars::new();
        self.exposes.iter().for_each(|var| {
            vars.set(&var.name, var.value.clone());
        });
        vars
    }

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
        self
    }

    pub fn with_catch(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.catches.push(build(step));
        self
    }

    pub fn with_timeout(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.timeouts.push(build(step));
        self
    }

    pub fn with_metadata<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.metadata.set(name, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn other_assign_keys_are_named_pools() {
        let assign: Assign = serde_json::from_value(json!({
            "owner": "alice",
            "candidates": { "params": { "kind": "role", "name": "reviewer" } },
            "cc": "bob",
            "my_name": {
                "params": { "kind": "context", "var": { "type": "current_actor" } }
            }
        }))
        .unwrap();
        assert_eq!(assign.owner.as_deref(), Some("alice"));
        assert_eq!(
            assign.candidates.as_ref().unwrap().params,
            CandidateExpr::Role {
                name: "reviewer".to_string()
            }
        );
        assert_eq!(
            assign.others["cc"],
            Candidates {
                params: CandidateExpr::Value("bob".to_string()),
                uses: USERS_PACKAGE.to_string(),
            }
        );
        assert_eq!(
            assign.others["my_name"].params,
            CandidateExpr::Context {
                var: crate::model::ContextVar::CurrentActor
            }
        );
        assert_eq!(assign.others["my_name"].uses, USERS_PACKAGE);
    }

    #[test]
    fn a_named_pool_accepts_a_bare_kind_object_and_a_custom_uses() {
        let assign: Assign = serde_json::from_value(json!({
            "cc": { "kind": "context", "var": { "type": "initiator" } },
            "approvers": {
                "uses": "corp.directory",
                "params": { "kind": "role", "name": "approver" }
            }
        }))
        .unwrap();
        assert_eq!(
            assign.others["cc"].params,
            CandidateExpr::Context {
                var: crate::model::ContextVar::Initiator
            }
        );
        assert_eq!(assign.others["cc"].uses, USERS_PACKAGE);
        assert_eq!(assign.others["approvers"].uses, "corp.directory");
    }

    #[test]
    fn assign_round_trips_named_pools_and_owner_stays_a_field() {
        let assign: Assign = serde_json::from_value(json!({
            "owner": "alice",
            "cc": "bob"
        }))
        .unwrap();
        let round = serde_json::to_value(&assign).unwrap();
        assert_eq!(round["owner"], json!("alice"));
        assert_eq!(
            round["cc"],
            json!({ "params": "bob", "uses": USERS_PACKAGE })
        );
        assert_eq!(serde_json::to_value(Assign::default()).unwrap(), json!({}));
        let parsed: Assign = serde_json::from_value(round).unwrap();
        assert_eq!(parsed, assign);
    }
}
