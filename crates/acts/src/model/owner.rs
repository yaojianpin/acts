//! The owner expression language: who a human act is assigned to, and the
//! participants a multi-approver step fans out to.
//!
//! [`OwnerExpr`] is data, not an implementation. The engine resolves its local
//! atoms (`user`, `context`, user-valued variables, and the set operations);
//! everything that needs business data (roles, groups, resource owners and
//! relations) is sent to the business system by the `acts.core.owner` IRQ
//! package, which answers with the participants. An expression that names one
//! of those atoms in `assign` fails with a message that points at the package
//! instead of silently assigning nobody.

use crate::{ActError, Result};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;
use std::fmt;

/// One resolved participant: the identity the assignment and approval
/// containers use, plus the display fields a business directory answers.
///
/// A bare string deserializes to `{id: <string>, code: <string>, name:
/// <string>}`; an object may omit `code`/`name`, which then default to the
/// `id`. The serialized shape always carries all three fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Participant {
    /// The stable business user id a task's `__assignment.owner` stores.
    pub id: String,
    /// The business code (login/employee number) of the user.
    pub code: String,
    /// The display name of the user.
    pub name: String,
}

impl Participant {
    /// A participant whose code and name are its id — what a bare id means
    /// when no directory record describes it.
    pub fn new(id: &str) -> Self {
        let id = id.trim();
        Self {
            id: id.to_string(),
            code: id.to_string(),
            name: id.to_string(),
        }
    }

    /// A participant with an explicit code and name. Empty values fall back to
    /// the id, so a directory never erases a display field by omission.
    pub fn with_names(id: &str, code: &str, name: &str) -> Self {
        let id = id.trim();
        let fallback = || id.to_string();
        Self {
            id: id.to_string(),
            code: if code.trim().is_empty() {
                fallback()
            } else {
                code.to_string()
            },
            name: if name.trim().is_empty() {
                fallback()
            } else {
                name.to_string()
            },
        }
    }

    /// Whether this participant names a user. An empty id can never be stored
    /// as an assignment or addressed as a decision maker.
    pub fn is_valid(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl<'de> Deserialize<'de> for Participant {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Record {
            id: String,
            #[serde(default)]
            code: Option<String>,
            #[serde(default)]
            name: Option<String>,
        }

        struct ParticipantVisitor;

        impl<'de> Visitor<'de> for ParticipantVisitor {
            type Value = Participant;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a participant id or an object with id/code/name")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                Ok(Participant::new(value))
            }

            fn visit_string<E: de::Error>(
                self,
                value: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(Participant::new(&value))
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let record = Record::deserialize(de::value::MapAccessDeserializer::new(map))?;
                let id = record.id.trim().to_string();
                Ok(Participant {
                    code: record.code.unwrap_or_else(|| id.clone()),
                    name: record.name.unwrap_or_else(|| id.clone()),
                    id,
                })
            }
        }

        deserializer.deserialize_any(ParticipantVisitor)
    }
}

/// One business resource a duty is resolved against.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
pub enum ResourceRef {
    Project {
        name: String,
    },
    Department {
        name: String,
    },
    /// A company/unit (`单位`).
    Unit {
        name: String,
    },
}

impl ResourceRef {
    /// The resource kind, as the wire spells it (`project`, `department`,
    /// `unit`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Project { .. } => "project",
            Self::Department { .. } => "department",
            Self::Unit { .. } => "unit",
        }
    }

    /// The resource's name.
    pub fn name(&self) -> &str {
        match self {
            Self::Project { name } | Self::Department { name } | Self::Unit { name } => name,
        }
    }

    /// Parse the wire kind and a name into a resource reference.
    pub fn new(kind: &str, name: &str) -> Result<Self> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ActError::Model(format!(
                "resource '{kind}' must name a resource, not an empty value"
            )));
        }
        match kind.trim() {
            "project" => Ok(Self::Project {
                name: name.to_string(),
            }),
            "department" => Ok(Self::Department {
                name: name.to_string(),
            }),
            "unit" => Ok(Self::Unit {
                name: name.to_string(),
            }),
            other => Err(ActError::Model(format!(
                "unknown resource kind '{other}'; expected project, department or unit"
            ))),
        }
    }
}

/// The duty a resource owner is selected by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Duty {
    /// Every owner of the resource, whatever the duty.
    #[default]
    All,
    /// The principal/head (`正职`).
    Principal,
    /// A deputy (`副职`).
    Deputy,
    /// A member (`成员`).
    Member,
    /// A deployment-defined duty; the directory decides what it means.
    Custom(String),
}

impl Duty {
    /// Whether this duty selects every owner.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }
}

/// The type a `ContextVar::Var` names. User/Users read the value as a person
/// or a person list and resolve locally; Role/Group and Project/Department/Unit
/// name business data and are resolved by the business system through the
/// `acts.core.owner` IRQ package (they are refused in `assign`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VarType {
    #[default]
    User,
    Users,
    Role,
    Group,
    Project,
    Department,
    Unit,
}

/// The named subject → subject / subject → resource relations the engine
/// understands. A deployment may add its own through [`Relation::Ref`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationName {
    /// The department a user belongs to.
    DepartmentOf,
    /// The unit a user belongs to.
    UnitOf,
    /// The project a user belongs to.
    ProjectOf,
    /// The user's manager (`上级`).
    ManagerOf,
    /// The user's reports (`下属`).
    ReportsOf,
    /// The user's assistant.
    AssistantOf,
}

impl RelationName {
    /// The wire word of the relation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DepartmentOf => "department_of",
            Self::UnitOf => "unit_of",
            Self::ProjectOf => "project_of",
            Self::ManagerOf => "manager_of",
            Self::ReportsOf => "reports_of",
            Self::AssistantOf => "assistant_of",
        }
    }
}

/// A deployment-defined relation: `{"rel": "resource", "value":
/// "department_of"}` names the `department_of` relation in the `resource`
/// relation namespace. The directory decides what the pair means.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationRef {
    pub rel: String,
    pub value: String,
}

/// One relation step: a named relation, or a deployment-defined reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Relation {
    Named(RelationName),
    Ref(RelationRef),
}

impl Relation {
    /// The relation's wire name, which is what a directory matches on.
    pub fn name(&self) -> &str {
        match self {
            Self::Named(name) => name.as_str(),
            Self::Ref(reference) => &reference.value,
        }
    }
}

/// The runtime variables an owner expression can read. They are resolved by
/// the engine, not by the directory, so they need no business data:
///
/// - `initiator`: the run's caller (the process's sealed subject).
/// - `current_actor`: the subject of the most recent decision on the acting
///   execution or its nearest ancestor; the initiator when the run has not
///   decided anything yet.
/// - `previous_approver`: the decision before the current actor;
///   `previous_approver_nth{back}` addresses older decisions.
/// - `var`: a workflow variable, read through the expression environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextVar {
    Initiator,
    CurrentActor,
    PreviousApprover,
    PreviousApproverNth {
        back: u32,
    },
    Var {
        name: String,
        #[serde(default)]
        ty: VarType,
        #[serde(default)]
        duty: Duty,
    },
}

/// The owner expression language.
///
/// A plain string is the legacy form — a literal user id or an expression
/// (`"${{ reviewer }}"`) resolved through the engine's expression path. The
/// object forms are tagged by `kind`:
///
/// ```json
/// { "kind": "context", "var": { "type": "current_actor" } }
/// { "kind": "owners_of",
///   "resource": { "resource": "department", "name": "研发部" },
///   "duty": "principal" }
/// { "kind": "relate",
///   "from": { "kind": "context", "var": { "type": "current_actor" } },
///   "rel": "department_of",
///   "duty": "principal" }
/// ```
///
/// Set expressions (`any_of`, `all_of`, `except`) evaluate to participant
/// sets. `assign.owner` requires exactly one result; the `acts.core.owner`
/// package returns the whole set for `block`/``parallel`/`sequence` fan-out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OwnerExpr {
    /// A literal user id or an engine expression string.
    Value(String),
    User {
        name: String,
    },
    Role {
        name: String,
    },
    Group {
        name: String,
    },
    OwnersOf {
        resource: ResourceRef,
        duty: Duty,
    },
    Context {
        var: ContextVar,
    },
    Relate {
        from: Box<OwnerExpr>,
        rel: Relation,
        duty: Option<Duty>,
    },
    AnyOf {
        operands: Vec<OwnerExpr>,
    },
    AllOf {
        operands: Vec<OwnerExpr>,
    },
    Except {
        base: Box<OwnerExpr>,
        minus: Box<OwnerExpr>,
    },
}

impl Default for OwnerExpr {
    fn default() -> Self {
        Self::Value(String::new())
    }
}

impl From<&str> for OwnerExpr {
    fn from(value: &str) -> Self {
        Self::Value(value.to_string())
    }
}

impl From<String> for OwnerExpr {
    fn from(value: String) -> Self {
        Self::Value(value)
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum OwnerExprSer<'a> {
    User {
        name: &'a str,
    },
    Role {
        name: &'a str,
    },
    Group {
        name: &'a str,
    },
    OwnersOf {
        resource: &'a ResourceRef,
        duty: &'a Duty,
    },
    Context {
        var: &'a ContextVar,
    },
    Relate {
        from: &'a OwnerExpr,
        rel: &'a Relation,
        #[serde(skip_serializing_if = "Option::is_none")]
        duty: Option<&'a Duty>,
    },
    AnyOf {
        operands: &'a [OwnerExpr],
    },
    AllOf {
        operands: &'a [OwnerExpr],
    },
    Except {
        base: &'a OwnerExpr,
        minus: &'a OwnerExpr,
    },
}

impl Serialize for OwnerExpr {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Value(value) => serializer.serialize_str(value),
            Self::User { name } => OwnerExprSer::User { name }.serialize(serializer),
            Self::Role { name } => OwnerExprSer::Role { name }.serialize(serializer),
            Self::Group { name } => OwnerExprSer::Group { name }.serialize(serializer),
            Self::OwnersOf { resource, duty } => {
                OwnerExprSer::OwnersOf { resource, duty }.serialize(serializer)
            }
            Self::Context { var } => OwnerExprSer::Context { var }.serialize(serializer),
            Self::Relate { from, rel, duty } => OwnerExprSer::Relate {
                from: from.as_ref(),
                rel,
                duty: duty.as_ref(),
            }
            .serialize(serializer),
            Self::AnyOf { operands } => OwnerExprSer::AnyOf {
                operands: operands.as_slice(),
            }
            .serialize(serializer),
            Self::AllOf { operands } => OwnerExprSer::AllOf {
                operands: operands.as_slice(),
            }
            .serialize(serializer),
            Self::Except { base, minus } => OwnerExprSer::Except {
                base: base.as_ref(),
                minus: minus.as_ref(),
            }
            .serialize(serializer),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum OwnerExprDe {
    User {
        name: String,
    },
    Role {
        name: String,
    },
    Group {
        name: String,
    },
    OwnersOf {
        resource: ResourceRef,
        #[serde(default)]
        duty: Duty,
    },
    Context {
        var: ContextVar,
    },
    Relate {
        from: Box<OwnerExpr>,
        rel: Relation,
        #[serde(default)]
        duty: Option<Duty>,
    },
    AnyOf {
        operands: Vec<OwnerExpr>,
    },
    AllOf {
        operands: Vec<OwnerExpr>,
    },
    Except {
        base: Box<OwnerExpr>,
        minus: Box<OwnerExpr>,
    },
}

impl From<OwnerExprDe> for OwnerExpr {
    fn from(value: OwnerExprDe) -> Self {
        match value {
            OwnerExprDe::User { name } => Self::User { name },
            OwnerExprDe::Role { name } => Self::Role { name },
            OwnerExprDe::Group { name } => Self::Group { name },
            OwnerExprDe::OwnersOf { resource, duty } => Self::OwnersOf { resource, duty },
            OwnerExprDe::Context { var } => Self::Context { var },
            OwnerExprDe::Relate { from, rel, duty } => Self::Relate { from, rel, duty },
            OwnerExprDe::AnyOf { operands } => Self::AnyOf { operands },
            OwnerExprDe::AllOf { operands } => Self::AllOf { operands },
            OwnerExprDe::Except { base, minus } => Self::Except { base, minus },
        }
    }
}

impl<'de> Deserialize<'de> for OwnerExpr {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OwnerExprVisitor;

        impl<'de> Visitor<'de> for OwnerExprVisitor {
            type Value = OwnerExpr;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an owner expression string or an object with a 'kind'")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                Ok(OwnerExpr::Value(value.to_string()))
            }

            fn visit_string<E: de::Error>(
                self,
                value: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(OwnerExpr::Value(value))
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                OwnerExprDe::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(OwnerExpr::from)
            }
        }

        deserializer.deserialize_any(OwnerExprVisitor)
    }
}

/// Deserialize a participant list from the JSON shapes a business system or an
/// expression may produce: an id, an object, or an array of either. `null` and
/// an absent value resolve to no participants.
pub(crate) fn participants_from_json(value: &JsonValue) -> Result<Vec<Participant>> {
    match value {
        JsonValue::Null => Ok(Vec::new()),
        JsonValue::String(value) if value.trim().is_empty() => Ok(Vec::new()),
        JsonValue::String(_) | JsonValue::Object(_) => {
            serde_json::from_value::<Participant>(value.clone())
                .map(|participant| vec![participant])
                .map_err(|error| ActError::Action(format!("invalid owner participant: {error}")))
        }
        JsonValue::Array(values) => {
            let mut participants = Vec::with_capacity(values.len());
            for value in values {
                participants.extend(participants_from_json(value)?);
            }
            Ok(participants)
        }
        other => Err(ActError::Action(format!(
            "owner expression resolved to a {}, expected a user id, a participant object or an array",
            json_type(other)
        ))),
    }
}

fn json_type(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plain_string_is_the_legacy_owner_expression() {
        let expr: OwnerExpr = serde_json::from_value(json!("${{ reviewer }}")).unwrap();
        assert_eq!(expr, OwnerExpr::Value("${{ reviewer }}".to_string()));
        assert_eq!(
            serde_json::to_value(&expr).unwrap(),
            json!("${{ reviewer }}")
        );
    }

    #[test]
    fn the_documented_owner_expressions_round_trip() {
        let any_of = json!({
            "kind": "any_of",
            "operands": [
                { "kind": "context", "var": { "type": "initiator" } },
                { "kind": "all_of", "operands": [
                    { "kind": "owners_of",
                      "resource": { "resource": "department", "name": "研发部" },
                      "duty": "principal" },
                    { "kind": "context",
                      "var": { "type": "var", "name": "owner_unit", "ty": "unit", "duty": "principal" } }
                ]},
                { "kind": "except",
                  "base": { "kind": "owners_of",
                            "resource": { "resource": "project", "name": "P1" },
                            "duty": "deputy" },
                  "minus": { "kind": "context", "var": { "type": "current_actor" } } }
            ]
        });
        let expr: OwnerExpr = serde_json::from_value(any_of.clone()).unwrap();
        assert!(matches!(expr, OwnerExpr::AnyOf { .. }));
        assert_eq!(serde_json::to_value(&expr).unwrap(), any_of);

        let relate = json!({
            "kind": "relate",
            "from": { "kind": "context", "var": { "type": "current_actor" } },
            "rel": { "rel": "resource", "value": "department_of" },
            "duty": "principal"
        });
        let expr: OwnerExpr = serde_json::from_value(relate.clone()).unwrap();
        match &expr {
            OwnerExpr::Relate { rel, duty, .. } => {
                assert_eq!(rel.name(), "department_of");
                assert_eq!(duty, &Some(Duty::Principal));
            }
            other => panic!("expected relate, got {other:?}"),
        }
        assert_eq!(serde_json::to_value(&expr).unwrap(), relate);
    }

    #[test]
    fn a_named_relation_is_a_string_and_a_custom_one_is_an_object() {
        let named: Relation = serde_json::from_value(json!("department_of")).unwrap();
        assert_eq!(named, Relation::Named(RelationName::DepartmentOf));
        assert_eq!(
            serde_json::to_value(&named).unwrap(),
            json!("department_of")
        );

        let custom: Relation =
            serde_json::from_value(json!({ "rel": "resource", "value": "department_of" })).unwrap();
        assert_eq!(custom.name(), "department_of");
        assert!(serde_json::to_value(&custom).unwrap().get("rel").is_some());
    }

    #[test]
    fn a_participant_accepts_an_id_or_a_record() {
        let id: Participant = serde_json::from_value(json!("u1")).unwrap();
        assert_eq!(id, Participant::new("u1"));
        assert_eq!(
            serde_json::to_value(&id).unwrap(),
            json!({"id": "u1", "code": "u1", "name": "u1"})
        );

        let record: Participant = serde_json::from_value(
            json!({ "id": "u2", "code": "alice", "name": "Alice", "extra": 1 }),
        )
        .unwrap();
        assert_eq!(record, Participant::with_names("u2", "alice", "Alice"));
        // an omitted display field defaults to the id
        let sparse: Participant = serde_json::from_value(json!({ "id": "u3" })).unwrap();
        assert_eq!(sparse.code, "u3");
        assert_eq!(sparse.name, "u3");
    }

    #[test]
    fn a_participant_list_flattens_arrays_and_rejects_scalars() {
        let participants =
            participants_from_json(&json!(["u1", { "id": "u2", "name": "Bob" }, null])).unwrap();
        assert_eq!(participants.len(), 2);
        assert_eq!(participants[1].name, "Bob");
        assert!(participants_from_json(&json!(1)).is_err());
    }

    #[test]
    fn a_resource_reference_parses_its_wire_kind() {
        assert_eq!(
            ResourceRef::new("department", "研发部").unwrap(),
            ResourceRef::Department {
                name: "研发部".to_string()
            }
        );
        assert!(ResourceRef::new("team", "x").is_err());
        assert!(ResourceRef::new("unit", " ").is_err());
        let value = serde_json::to_value(ResourceRef::Project {
            name: "P1".to_string(),
        })
        .unwrap();
        assert_eq!(value, json!({"resource": "project", "name": "P1"}));
    }

    #[test]
    fn a_context_var_reads_the_documented_tags() {
        let var: ContextVar =
            serde_json::from_value(json!({ "type": "previous_approver_nth", "back": 2 })).unwrap();
        assert_eq!(var, ContextVar::PreviousApproverNth { back: 2 });

        let var: ContextVar = serde_json::from_value(
            json!({ "type": "var", "name": "owner_unit", "ty": "unit", "duty": "principal" }),
        )
        .unwrap();
        assert_eq!(
            var,
            ContextVar::Var {
                name: "owner_unit".to_string(),
                ty: VarType::Unit,
                duty: Duty::Principal,
            }
        );
        // the defaults are a single user, any duty
        let var: ContextVar =
            serde_json::from_value(json!({ "type": "var", "name": "reviewer" })).unwrap();
        assert_eq!(
            var,
            ContextVar::Var {
                name: "reviewer".to_string(),
                ty: VarType::User,
                duty: Duty::All,
            }
        );
    }
}
