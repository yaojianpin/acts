//! The `$assign` read projection: the assignment, delegation and computed
//! candidate pool of one execution as the object an expression reads.
//!
//! The projection is read-only and never fails: an execution (or a chain of
//! them) without assign facts answers `null` fields and an empty pool, so a
//! flow that reads `$assign.candidates` without declaring one sees an empty
//! list instead of an unknown-variable error.
//!
//! Resolution follows the scope chain, one field family at a time:
//!
//! - `owner`/`assignee` (and the delegation's `delegator`/`delegatee`) come
//!   from the nearest execution carrying `__assignment`; that same execution
//!   answers the delegation, so a child never inherits an unrelated
//!   ancestor's delegatee.
//! - `candidates`/`ids`/`participants`/`count` come from the nearest
//!   execution whose pool was computed (`__candidates`) — a container's
//!   generated child therefore reads the pool that generated it, even though
//!   the child has its own assignment.
//!
//! `$assign.at("<step-id>")` names an execution explicitly (a node id answers
//! its newest generation, a wire tid the exact instance) and reads that
//! execution's own facts, without walking its ancestors.
//!
//! Every function here is a method on [`Assign`](super::Assign), like the
//! rest of the capability.

use super::candidates::CANDIDATES_VAR;
use super::*;
use crate::scheduler::core::commit::aggregate::ProcessSnapshot;
use crate::scheduler::core::model::task_graph::TaskId;
use acts_expr::{Map, Value};
use serde_json::Value as JsonValue;
use std::sync::Arc;

impl Assign {
    /// The `$assign` object for one evaluation anchor.
    ///
    /// `snapshot` is the shared aggregate the scope reads (an effect scope
    /// carries one; a scope built without it answers the empty projection).
    pub(crate) fn projection(
        snapshot: Option<Arc<ProcessSnapshot>>,
        process_id: &str,
        current: &TaskId,
    ) -> Value {
        let fields = match snapshot.as_deref() {
            Some(snapshot) => Self::nearest_fields(snapshot, current),
            None => Self::empty_map(),
        };
        Self::assemble(fields, snapshot, process_id.to_string())
    }

    /// The projection outside a task context: the same shape, all empty.
    pub(crate) fn empty_projection() -> Value {
        Self::assemble(Self::empty_map(), None, String::new())
    }

    /// Attach the explicit `$assign.at(...)` lookup to the field map.
    fn assemble(
        mut fields: Map,
        snapshot: Option<Arc<ProcessSnapshot>>,
        process_id: String,
    ) -> Value {
        fields.insert(
            "at".to_string(),
            Value::function(move |args: &[Value]| {
                let Some(raw) = args.iter().find_map(Value::as_str) else {
                    return Ok(Value::Null);
                };
                let Some(snapshot) = snapshot.as_deref() else {
                    return Ok(Self::object(Self::empty_map()));
                };
                Ok(match Self::execution_named(snapshot, &process_id, raw) {
                    Some(task) => Self::object(Self::own_fields(snapshot, &task)),
                    None => Self::object(Self::empty_map()),
                })
            }),
        );
        Self::object(fields)
    }

    fn object(fields: Map) -> Value {
        Value::Map(Arc::new(fields))
    }

    /// The empty shape: `null` identities and an empty pool, never an error.
    fn empty_map() -> Map {
        let mut fields = Map::new();
        for name in ["owner", "assignee", "delegator", "delegatee"] {
            fields.insert(name.to_string(), Value::Null);
        }
        for name in ["candidates", "ids", "participants"] {
            fields.insert(name.to_string(), Value::list(Vec::<Value>::new()));
        }
        fields.insert("count".to_string(), Value::Null);
        fields
    }

    /// The nearest assignment along the scope chain, with that execution's
    /// own delegation (a missing or revoked record answers `None`).
    fn nearest_assignment(
        snapshot: &ProcessSnapshot,
        current: &TaskId,
    ) -> Option<(Assignment, Option<Delegation>)> {
        let mut cursor = Some(current.clone());
        while let Some(id) = cursor {
            if let Some(assignment) = Assignment::of(snapshot, &id).ok().flatten() {
                let delegation = Delegation::of(snapshot, &id).ok().flatten();
                return Some((assignment, delegation));
            }
            cursor = snapshot.tasks.get(&id).and_then(|task| task.parent.clone());
        }
        None
    }

    /// The nearest computed pool along the scope chain, independently of the
    /// assignment walk: a generated child carries its own assignment and
    /// still reads the pool of the container that generated it.
    fn nearest_pool(snapshot: &ProcessSnapshot, current: &TaskId) -> Option<JsonValue> {
        let mut cursor = Some(current.clone());
        while let Some(id) = cursor {
            if let Some(pool) = Self::own_pool(snapshot, &id) {
                return Some(pool);
            }
            cursor = snapshot.tasks.get(&id).and_then(|task| task.parent.clone());
        }
        None
    }

    /// The assignment and pool facts of the acting execution and its nearest
    /// assignment ancestor, as expression fields.
    fn nearest_fields(snapshot: &ProcessSnapshot, current: &TaskId) -> Map {
        let mut fields = Self::empty_map();
        if let Some((assignment, delegation)) = Self::nearest_assignment(snapshot, current) {
            Self::insert_assignment(&mut fields, &assignment, delegation.as_ref());
        }
        if let Some(pool) = Self::nearest_pool(snapshot, current) {
            Self::insert_pool(&mut fields, &pool);
        }
        fields
    }

    /// One named execution's own facts — no ancestor walk.
    fn own_fields(snapshot: &ProcessSnapshot, task: &TaskId) -> Map {
        let mut fields = Self::empty_map();
        if let Ok(Some(assignment)) = Assignment::of(snapshot, task) {
            let delegation = Delegation::of(snapshot, task).ok().flatten();
            Self::insert_assignment(&mut fields, &assignment, delegation.as_ref());
        }
        if let Some(pool) = Self::own_pool(snapshot, task) {
            Self::insert_pool(&mut fields, &pool);
        }
        fields
    }

    fn insert_assignment(
        fields: &mut Map,
        assignment: &Assignment,
        delegation: Option<&Delegation>,
    ) {
        fields.insert("owner".to_string(), Value::from(assignment.owner.clone()));
        fields.insert(
            "assignee".to_string(),
            Value::from(assignment.assignee.clone()),
        );
        if let Some(delegation) = delegation {
            fields.insert(
                "delegator".to_string(),
                Value::from(delegation.delegator.clone()),
            );
            fields.insert(
                "delegatee".to_string(),
                Value::from(delegation.delegatee.clone()),
            );
        }
    }

    fn insert_pool(fields: &mut Map, pool: &JsonValue) {
        let participants = pool
            .get("participants")
            .filter(|value| value.is_array())
            .cloned()
            .unwrap_or_else(|| JsonValue::Array(Vec::new()));
        let ids = pool
            .get("ids")
            .filter(|value| value.is_array())
            .cloned()
            .unwrap_or_else(|| JsonValue::Array(Vec::new()));
        let count = pool.get("count").cloned().unwrap_or(JsonValue::Null);
        // `candidates` is the canonical list a fan-out and an owner
        // expression read; `ids` is the same list under the answer's name.
        fields.insert("candidates".to_string(), Value::from(ids.clone()));
        fields.insert("ids".to_string(), Value::from(ids));
        fields.insert("participants".to_string(), Value::from(participants));
        fields.insert("count".to_string(), Value::from(count));
    }

    /// One execution's computed pool marker, when it holds one.
    fn own_pool(snapshot: &ProcessSnapshot, task: &TaskId) -> Option<JsonValue> {
        let raw = snapshot
            .variables
            .get(&(task.clone(), CANDIDATES_VAR.to_string()))?;
        serde_json::from_str::<JsonValue>(raw)
            .ok()
            .filter(JsonValue::is_object)
    }

    /// Resolve an explicit `$assign.at(...)` argument: a wire tid names the
    /// exact instance, a node id its newest generation.
    fn execution_named(snapshot: &ProcessSnapshot, process_id: &str, raw: &str) -> Option<TaskId> {
        if let Some(task) = snapshot
            .tasks
            .keys()
            .find(|task| task.wire(process_id) == raw)
        {
            return Some(task.clone());
        }
        snapshot
            .tasks
            .keys()
            .filter(|task| task.node.as_str() == raw)
            .max_by_key(|task| task.generation)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::TaskSnapshot;
    use crate::scheduler::core::model::task_graph::TaskState;
    use crate::scheduler::shell::model::ProcessSpec;
    use serde_json::json;

    fn snapshot() -> ProcessSnapshot {
        ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap()
    }

    fn child(snapshot: &mut ProcessSnapshot) -> TaskId {
        let parent = TaskId::new("a", 0).unwrap();
        let child = TaskId::new("a#item", 0).unwrap();
        snapshot.tasks.insert(
            child.clone(),
            TaskSnapshot {
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("a#item")
                    .unwrap(),
                parent: Some(parent),
                predecessors: Vec::new(),
                state: TaskState::Running,
                revision: 0,
            },
        );
        child
    }

    fn eval(value: Value, expr: &str) -> JsonValue {
        let mut context = acts_expr::Context::new();
        context.set("$assign", value);
        acts_expr::Expr::compile(expr)
            .unwrap()
            .eval(&context)
            .unwrap()
            .to_json()
            .unwrap()
    }

    /// Identity fields come from the nearest assignment and that execution's
    /// own delegation; the pool walks independently, so the child reads the
    /// parent's pool while an unrelated parent delegation never leaks.
    #[test]
    fn projection_resolves_each_field_family_along_the_chain() {
        let mut snapshot = snapshot();
        let parent = TaskId::new("a", 0).unwrap();
        let child = child(&mut snapshot);
        snapshot.variables.insert(
            (parent.clone(), ASSIGNMENT_VAR.to_string()),
            json!({"owner": "boss", "assignee": "boss", "rev": 2}).to_string(),
        );
        snapshot.variables.insert(
            (parent.clone(), DELEGATION_VAR.to_string()),
            json!({"id": "dg1", "delegator": "boss", "delegatee": "aide"}).to_string(),
        );
        snapshot.variables.insert(
            (parent.clone(), CANDIDATES_VAR.to_string()),
            json!({"participants": [{"id": "u1"}], "ids": ["u1", "u2"], "count": 2}).to_string(),
        );
        snapshot.variables.insert(
            (child.clone(), ASSIGNMENT_VAR.to_string()),
            json!({"owner": "u7", "assignee": "u8", "rev": 3}).to_string(),
        );

        let value = Assign::projection(Some(Arc::new(snapshot)), "p", &child);
        assert_eq!(eval(value.clone(), "$assign.owner"), json!("u7"));
        assert_eq!(eval(value.clone(), "$assign.assignee"), json!("u8"));
        // The child has no delegation of its own: the parent's never leaks.
        assert_eq!(eval(value.clone(), "$assign.delegator"), json!(null));
        assert_eq!(eval(value.clone(), "$assign.delegatee"), json!(null));
        assert_eq!(
            eval(value.clone(), "$assign.candidates"),
            json!(["u1", "u2"])
        );
        assert_eq!(eval(value.clone(), "$assign.ids"), json!(["u1", "u2"]));
        assert_eq!(eval(value.clone(), "$assign.count"), json!(2));

        // The explicit form reads the named execution's own facts only.
        assert_eq!(eval(value.clone(), "$assign.at('a').owner"), json!("boss"));
        assert_eq!(
            eval(value.clone(), "$assign.at('a').delegator"),
            json!("boss")
        );
        assert_eq!(
            eval(value.clone(), "$assign.at('a#item').owner"),
            json!("u7")
        );
        let wire = child.wire("p");
        assert_eq!(
            eval(value.clone(), &format!("$assign.at('{wire}').owner")),
            json!("u7")
        );
        assert_eq!(eval(value, "$assign.at('missing').candidates"), json!([]));
    }

    /// No assign fact anywhere answers nulls and empty lists, never an error.
    #[test]
    fn projection_without_assign_facts_is_empty() {
        let mut snapshot = snapshot();
        let child = child(&mut snapshot);
        let value = Assign::projection(Some(Arc::new(snapshot)), "p", &child);
        assert_eq!(eval(value.clone(), "$assign.owner"), json!(null));
        assert_eq!(eval(value.clone(), "$assign.candidates"), json!([]));
        assert_eq!(eval(value, "$assign.count"), json!(null));
    }
}
