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
//! - `owner`/`assignee` (and the delegation's `delegator`) come from the
//!   nearest execution carrying `#assignment`; `assignee` is whoever
//!   executes the act now (the delegatee while a delegation is active), so
//!   a child never inherits an unrelated ancestor's delegation.
//! - `candidates`/`ids`/`participants`/`count` come from the nearest
//!   execution whose pool was computed (`#candidates`) — a container's
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
        for name in ["owner", "assignee", "delegator"] {
            fields.insert(name.to_string(), Value::Null);
        }
        for name in ["ids", "participants"] {
            fields.insert(name.to_string(), Value::list(Vec::<Value>::new()));
        }
        fields.insert("count".to_string(), Value::Null);
        fields.insert(
            "candidates".to_string(),
            Self::pool_object(&JsonValue::Null),
        );
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
    /// assignment ancestor, as expression fields. Named pools
    /// (`assign.<key>`) ride the same scope chain; a nearer answer wins a
    /// duplicate name.
    fn nearest_fields(snapshot: &ProcessSnapshot, current: &TaskId) -> Map {
        let mut fields = Self::empty_map();
        if let Some((assignment, delegation)) = Self::nearest_assignment(snapshot, current) {
            Self::insert_assignment(&mut fields, &assignment, delegation.as_ref());
        }
        if let Some(pool) = Self::nearest_pool(snapshot, current) {
            Self::insert_pool(&mut fields, &pool);
        }
        for (name, pool) in Self::nearest_named_pools(snapshot, current) {
            fields.insert(name, pool);
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
        for (name, pool) in Self::own_named_pools(snapshot, task) {
            fields.insert(name, Self::pool_object(&pool));
        }
        fields
    }

    /// The named pools (`#pool_<name>` markers) along the acting
    /// execution's scope chain, nearest answer first: the first entry per
    /// name wins. Each field is the pool object (`$assign.<name>.users`,
    /// `.ids`, `.count`).
    fn nearest_named_pools(snapshot: &ProcessSnapshot, current: &TaskId) -> Vec<(String, Value)> {
        let mut pools: Vec<(String, Value)> = Vec::new();
        let mut cursor = Some(current.clone());
        while let Some(id) = cursor {
            for (name, pool) in Self::own_named_pools(snapshot, &id) {
                if !pools.iter().any(|(existing, _)| *existing == name) {
                    pools.push((name, Self::pool_object(&pool)));
                }
            }
            cursor = snapshot.tasks.get(&id).and_then(|task| task.parent.clone());
        }
        pools
    }

    /// The computed named pools of one execution: every `#pool_<name>`
    /// marker except the wait markers, as `(name, pool JSON)`.
    fn own_named_pools(snapshot: &ProcessSnapshot, task: &TaskId) -> Vec<(String, JsonValue)> {
        let mut pools = Vec::new();
        for ((scope, name), raw) in snapshot.variables.iter() {
            if scope != task {
                continue;
            }
            let Some(key) = name
                .strip_prefix("#pool_")
                .filter(|key| !key.starts_with("wait_"))
            else {
                continue;
            };
            let Ok(pool) = serde_json::from_str::<JsonValue>(raw) else {
                continue;
            };
            if pool.is_object() {
                pools.push((key.to_string(), pool));
            }
        }
        pools
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
        }
    }

    /// The canonical pool's fields: the nested `candidates` object
    /// (`$assign.candidates.users` / `.ids` / `.count`) plus the legacy
    /// top-level `ids`/`participants`/`count` aliases the existing flows
    /// read.
    fn insert_pool(fields: &mut Map, pool: &JsonValue) {
        fields.insert("candidates".to_string(), Self::pool_object(pool));
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
        // `ids` is the canonical list under the answer's own name.
        fields.insert("ids".to_string(), Value::from(ids));
        fields.insert("participants".to_string(), Value::from(participants));
        fields.insert(
            "count".to_string(),
            Value::from(pool.get("count").cloned().unwrap_or(JsonValue::Null)),
        );
    }

    /// One pool as the object its key projects: the participant records
    /// (`users`), the id list (`ids`) and the answer's `count`. This is the
    /// shape under `$assign.candidates` and under every named key
    /// (`$assign.<name>`).
    fn pool_object(pool: &JsonValue) -> Value {
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
        let mut fields = Map::new();
        fields.insert("users".to_string(), Value::from(participants));
        fields.insert("ids".to_string(), Value::from(ids));
        fields.insert("count".to_string(), Value::from(count));
        Self::object(fields)
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
        assert_eq!(
            eval(value.clone(), "$assign.candidates.users"),
            json!([{ "id": "u1" }])
        );
        assert_eq!(
            eval(value.clone(), "$assign.candidates.ids"),
            json!(["u1", "u2"])
        );
        assert_eq!(eval(value.clone(), "$assign.candidates.count"), json!(2));
        // The legacy top-level aliases keep reading the canonical pool.
        assert_eq!(eval(value.clone(), "$assign.ids"), json!(["u1", "u2"]));
        assert_eq!(
            eval(value.clone(), "$assign.participants"),
            json!([{ "id": "u1" }])
        );
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
        assert_eq!(
            eval(value, "$assign.at('missing').candidates.ids"),
            json!([])
        );
    }

    /// A named pool rides the scope chain like the canonical one and
    /// projects under its own key with the same object shape; a nearer
    /// answer wins a duplicate name.
    #[test]
    fn projection_resolves_named_pools_under_their_keys() {
        {
            let mut snapshot = snapshot();
            let parent = TaskId::new("a", 0).unwrap();
            let child = child(&mut snapshot);
            snapshot.variables.insert(
                (parent.clone(), "#pool_cc".to_string()),
                json!({"participants": [{"id": "u9"}], "ids": ["u9"], "count": 1}).to_string(),
            );
            snapshot.variables.insert(
                (child.clone(), "#pool_cc".to_string()),
                json!({"participants": [{"id": "u3"}], "ids": ["u3"], "count": 1}).to_string(),
            );
            snapshot.variables.insert(
                (child.clone(), "#pool_wait_cc".to_string()),
                json!("x").to_string(),
            );

            let value = Assign::projection(Some(Arc::new(snapshot)), "p", &child);
            // The child's own answer wins the name; the wait marker is not a
            // pool.
            assert_eq!(eval(value.clone(), "$assign.cc.ids"), json!(["u3"]));
            assert_eq!(
                eval(value.clone(), "$assign.cc.users"),
                json!([{ "id": "u3" }])
            );
            assert_eq!(eval(value.clone(), "$assign.cc.count"), json!(1));
        }

        // The parent's scope chain answers with its own execution's pool.
        {
            let mut chain = snapshot();
            let top = TaskId::new("a", 0).unwrap();
            chain.variables.insert(
                (top.clone(), "#pool_cc".to_string()),
                json!({"participants": [{"id": "u9"}], "ids": ["u9"], "count": 1}).to_string(),
            );
            let value = Assign::projection(Some(Arc::new(chain)), "p", &top);
            assert_eq!(eval(value.clone(), "$assign.cc.ids"), json!(["u9"]));

            // The explicit form reads the named execution's own pool.
            let top = TaskId::new("a", 0).unwrap();
            let mut chain = snapshot();
            let inner = {
                let inner = TaskId::new("a#item", 0).unwrap();
                chain.tasks.insert(
                    inner.clone(),
                    TaskSnapshot {
                        model_node: crate::scheduler::core::model::model_graph::NodeId::parse(
                            "a#item",
                        )
                        .unwrap(),
                        parent: Some(top.clone()),
                        predecessors: Vec::new(),
                        state: TaskState::Running,
                        revision: 0,
                    },
                );
                inner
            };
            chain.variables.insert(
                (top.clone(), "#pool_cc".to_string()),
                json!({"participants": [{"id": "u9"}], "ids": ["u9"], "count": 1}).to_string(),
            );
            chain.variables.insert(
                (inner.clone(), "#pool_cc".to_string()),
                json!({"participants": [{"id": "u3"}], "ids": ["u3"], "count": 1}).to_string(),
            );
            let value = Assign::projection(Some(Arc::new(chain)), "p", &top);
            assert_eq!(
                eval(value.clone(), "$assign.at('a#item').cc.ids"),
                json!(["u3"])
            );
            assert_eq!(eval(value, "$assign.at('a').cc.ids"), json!(["u9"]));
        }
    }

    /// No assign fact anywhere answers nulls and empty pools, never an error.
    #[test]
    fn projection_without_assign_facts_is_empty() {
        let mut snapshot = snapshot();
        let child = child(&mut snapshot);
        let value = Assign::projection(Some(Arc::new(snapshot)), "p", &child);
        assert_eq!(eval(value.clone(), "$assign.owner"), json!(null));
        assert_eq!(eval(value.clone(), "$assign.candidates.ids"), json!([]));
        assert_eq!(eval(value.clone(), "$assign.candidates.users"), json!([]));
        assert_eq!(eval(value, "$assign.count"), json!(null));
    }
}
