use crate::{
    ActError, ActSchema, Branch, JoinRule, ModelBase, Result, Step, Trigger, Variant, Vars,
    scheduler::{
        BLOCK_PACKAGE, NodeContent, NodeData, PARALLEL_PACKAGE, SEQUENCE_PACKAGE, assign_ids,
    },
    utils::consts,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub env: Vec<Variant>,

    /// define the workflow global vars
    #[serde(default)]
    pub vars: Vec<Variant>,

    /// resource name the workflow operates on (`rn`), a colon-separated
    /// path such as `orders:eu` or `app:billing:invoices`. It is the name an
    /// acl user's `patterns` match: a user may deploy and start only the
    /// workflows whose `rn` falls within its grants. Empty means the model
    /// claims no resource, and only an unrestricted user may deploy or run
    /// it.
    #[serde(default)]
    pub rn: String,
    /// input json schema
    #[serde(default)]
    pub inputs: ActSchema,

    #[serde(default)]
    pub on: Vec<Trigger>,

    #[serde(default)]
    pub ver: String,

    /// extra options to send to client
    #[serde(default)]
    pub options: Vars,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,

    /// variables to expose
    #[serde(default)]
    pub exposes: Vec<Variant>,
}

impl Workflow {
    pub fn from_yml(s: &str) -> Result<Self> {
        serde_yaml::from_str::<Workflow>(s)
            .map_err(|error| ActError::Model(locate_yaml_error(s, &error)))
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let workflow = serde_json::from_str::<Workflow>(s);
        match workflow {
            Ok(v) => Ok(v),
            Err(e) => Err(ActError::Model(format!("{e}"))),
        }
    }

    pub fn set_env(&mut self, vars: &[Variant]) {
        for var in vars {
            self.env
                .iter_mut()
                .find(|v| v.name == var.name)
                .map(|v| v.value = var.value.clone())
                .unwrap_or_else(|| self.env.push(var.clone()));
        }
    }

    pub fn env(&self) -> Vars {
        let mut ret = Vars::new();
        self.env.iter().for_each(|var| {
            ret.set(&var.name, var.value.clone());
        });

        ret
    }

    pub fn set_vars(&mut self, vars: &Vars) {
        for (name, value) in vars.iter() {
            self.vars.push(Variant::create(name, value.clone()));
        }
    }

    pub fn vars(&self) -> Vars {
        let mut ret = Vars::new();
        self.vars.iter().for_each(|var| {
            ret.set(&var.name, var.value.clone());
        });

        ret
    }

    pub fn tree_output(&self) -> String {
        let model = self.prepared().unwrap();
        let mut output = String::new();
        let mut visited = HashSet::new();
        write_tree_output(
            &model,
            &model.id,
            true,
            &HashMap::new(),
            &mut visited,
            &mut output,
        );
        output
    }

    /// The persisted `tasks.node_data` shape of one model node, derived from
    /// the declaration. `$` (the root instance id) addresses the workflow
    /// node, like the kernel's root lookup did.
    pub(crate) fn node_data(&self, node_id: &str) -> Option<NodeData> {
        let target = if node_id == consts::TASK_ROOT_TID {
            self.id.as_str()
        } else {
            node_id
        };
        locate(self, target).map(|position| NodeData {
            id: position.content.id(),
            content: position.content,
            level: position.level,
            parent_id: position.parent_id,
            prev_id: position.prev_id,
            next_id: position.next_id,
        })
    }

    /// Validate the declaration as the kernel imports it: `rn`, triggers,
    /// duplicate node ids, `next` targets (which must already be declared),
    /// and `while` combined with `next`. Ids the model leaves unnamed are
    /// assigned on a copy first, exactly like the import.
    pub fn valid(&self) -> Result<()> {
        self.prepared().map(|_| ())
    }

    /// A copy with assigned ids, validated — the shape the import and the
    /// tree output read. The caller's model is never mutated.
    pub(crate) fn prepared(&self) -> Result<Workflow> {
        let mut model = self.clone();
        assign_ids(&mut model);
        model.validate()?;
        Ok(model)
    }

    fn validate(&self) -> Result<()> {
        // an `rn` is a literal resource name acl patterns match against, so
        // it may not be a glob, contain spaces, or carry empty segments —
        // a malformed one would silently match nothing
        if !self.rn.is_empty() {
            let rn = self.rn.trim();
            if rn.contains(|c: char| c.is_whitespace())
                || rn
                    .chars()
                    .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}'))
                || rn.split(':').any(str::is_empty)
            {
                return Err(ActError::Model(format!(
                    "invalid rn '{}' in model '{}': colon-separated segments, no spaces, no glob",
                    self.rn, self.id
                )));
            }
        }

        let mut on_ids = HashSet::new();
        for on in self.on.iter() {
            // validate trigger declarations (id/kind/config) — triggers are
            // not process nodes
            on.valid()?;
            if !on_ids.insert(on.id.as_str()) {
                return Err(ActError::Model(format!("dup event id with '{}'", on.id)));
            }
        }

        let mut built = HashSet::new();
        built.insert(self.id.clone());
        for step in self.steps.iter() {
            validate_step(step, &mut built)?;
        }
        Ok(())
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        match self.steps.iter().find(|s| s.id == id) {
            Some(s) => Some(s),
            None => None,
        }
    }
    pub fn set_id(&mut self, id: &str) {
        self.id = id.to_string();
    }

    pub fn set_ver(&mut self, ver: &str) {
        self.ver = ver.to_string();
    }

    pub fn to_yml(&self) -> Result<String> {
        match serde_yaml::to_string(self) {
            Ok(s) => Ok(s),
            Err(e) => Err(ActError::Model(e.to_string())),
        }
    }

    pub fn to_json(&self) -> Result<String> {
        match serde_json::to_string(self) {
            Ok(s) => Ok(s),
            Err(e) => Err(ActError::Model(e.to_string())),
        }
    }

    pub fn with_metadata<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.metadata.set(name, value);
        self
    }
}

impl ModelBase for Workflow {
    fn id(&self) -> &str {
        &self.id
    }
}

/// for builder
impl Workflow {
    pub fn new() -> Self {
        Self {
            ver: "0.1.0".to_string(),
            ..Default::default()
        }
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

    pub fn with_ver(mut self, ver: &str) -> Self {
        self.ver = ver.to_string();
        self
    }

    /// Set the resource name (`rn`) — see [`Workflow::rn`].
    pub fn with_rn(mut self, rn: &str) -> Self {
        self.rn = rn.to_string();
        self
    }
    pub fn with_option<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.options.set(name, value);
        self
    }

    pub fn with_var<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.vars.push(Variant::create(name, value));
        self
    }

    pub fn with_env<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.env
            .iter_mut()
            .find(|v| v.name == name)
            .map(|v| v.value = json!(value))
            .unwrap_or_else(|| self.env.push(Variant::create(name, value)));
        self
    }

    pub fn with_inputs(mut self, inputs: ActSchema) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_expose(mut self, var: Variant) -> Self {
        self.exposes.push(var);
        self
    }

    pub fn exposes(&self) -> Vars {
        let mut vars = Vars::new();
        self.exposes.iter().for_each(|var| {
            vars.set(&var.name, var.value.clone());
        });
        vars
    }

    /// Whether any step declares an assignment: an `assign.owner` or a pool
    /// (the canonical `assign.candidates` or a named key of `assign`).
    ///
    /// `assign.owner` is a participant primitive: a model that uses it needs
    /// the assignment resolver attached (the engine installs its own), and
    /// the publish path checks this before accepting the model.
    pub fn has_assignment(&self) -> bool {
        self.steps.iter().any(Step::has_assignment)
    }

    /// The capabilities this model requires, as capability types and optional
    /// provider keys. The publish path resolves each requirement against the
    /// installed providers and refuses the model when none can serve it.
    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
        self
    }

    pub fn with_trigger(mut self, build: fn(Trigger) -> Trigger) -> Self {
        let trigger = build(Trigger::new());
        self.on.push(trigger);
        self
    }
}

/// One declared node as the tree output and `node_data` read it: the model
/// content plus the declaration links the legacy tree build recorded.
struct Position {
    content: NodeContent,
    level: usize,
    parent_id: Option<String>,
    prev_id: Option<String>,
    next_id: Option<String>,
}

/// Resolve a declared node by id, computing its level and links from the
/// declaration order alone: a first child hangs off its parent, later
/// siblings link through `prev`/`next`, and a step's explicit `next`
/// replaces the declaration-order successor.
fn locate(model: &Workflow, target: &str) -> Option<Position> {
    if model.id == target {
        return Some(Position {
            content: NodeContent::Workflow(Arc::new(model.clone())),
            level: 0,
            parent_id: None,
            prev_id: None,
            // The root enters its first step through the children group, so
            // it has no `next` link of its own.
            next_id: None,
        });
    }
    locate_in_steps(&model.steps, 1, Some(&model.id), target)
}

fn locate_in_steps(
    steps: &[Step],
    level: usize,
    first_parent: Option<&str>,
    target: &str,
) -> Option<Position> {
    for (index, step) in steps.iter().enumerate() {
        let position = Position {
            content: NodeContent::Step(Box::new(step.clone())),
            level,
            parent_id: if index == 0 {
                first_parent.map(str::to_string)
            } else {
                None
            },
            prev_id: index.checked_sub(1).map(|i| steps[i].id.clone()),
            next_id: step
                .next
                .clone()
                .or_else(|| steps.get(index + 1).map(|next| next.id.clone())),
        };
        if step.id == target {
            return Some(position);
        }
        if step.next.is_none()
            && let Some(position) = locate_in_branches(&step.branches, level + 1, &step.id, target)
        {
            return Some(position);
        }
        if let Some(position) = locate_in_steps(&step.catches, level + 1, Some(&step.id), target) {
            return Some(position);
        }
        if let Some(position) = locate_in_steps(&step.timeouts, level + 1, Some(&step.id), target) {
            return Some(position);
        }
    }
    None
}

fn locate_in_branches(
    branches: &[Branch],
    level: usize,
    parent_id: &str,
    target: &str,
) -> Option<Position> {
    for branch in branches {
        if branch.id == target {
            return Some(Position {
                content: NodeContent::Branch(Box::new(branch.clone())),
                level,
                parent_id: Some(parent_id.to_string()),
                prev_id: None,
                // A branch is entered at its first declared step and has no
                // declaration-order successor of its own.
                next_id: None,
            });
        }
        if let Some(position) = locate_in_steps(&branch.steps, level + 1, Some(&branch.id), target)
        {
            return Some(position);
        }
    }
    None
}

fn validate_step(step: &Step, built: &mut HashSet<String>) -> Result<()> {
    if step.next.is_some() && step.r#while.is_some() {
        return Err(ActError::Runtime(format!(
            "step '{}' cannot combine 'while' with 'next': a while loop always flows to the next declared step when its condition fails",
            step.id
        )));
    }
    validate_join(step)?;
    if !built.insert(step.id.clone()) {
        return Err(ActError::Model(format!("dup node id with '{}'", step.id)));
    }

    match &step.next {
        Some(next) => {
            // The legacy build resolved `next` against the nodes already
            // declared, so a forward reference is refused.
            if !built.contains(next) {
                return Err(ActError::Runtime(format!(
                    "found next node error by '{next}'",
                )));
            }
        }
        None => {
            validate_branch_needs(step)?;
            for branch in &step.branches {
                if !built.insert(branch.id.clone()) {
                    return Err(ActError::Model(format!("dup node id with '{}'", branch.id)));
                }
                for inner in &branch.steps {
                    validate_step(inner, built)?;
                }
            }
        }
    }

    for catch in &step.catches {
        validate_step(catch, built)?;
    }
    for timeout in &step.timeouts {
        validate_step(timeout, built)?;
    }
    Ok(())
}

/// A branch's `needs` names sibling branches of the same step, and the
/// dependency graph must be acyclic — the kernel queues a dependent branch
/// until every predecessor is terminal, so a missing target or a cycle would
/// leave it `Created` forever instead of failing the start.
fn validate_branch_needs(step: &Step) -> Result<()> {
    let positions: HashMap<&str, usize> = step
        .branches
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.id.as_str(), index))
        .collect();
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); step.branches.len()];
    for (index, branch) in step.branches.iter().enumerate() {
        for need in &branch.needs {
            if need == &branch.id {
                return Err(ActError::Model(format!(
                    "branch '{}' in step '{}' needs itself",
                    branch.id, step.id
                )));
            }
            let dependency = positions.get(need.as_str()).copied().ok_or_else(|| {
                ActError::Model(format!(
                    "branch '{}' in step '{}' needs unknown branch '{}'",
                    branch.id, step.id, need
                ))
            })?;
            edges[index].push(dependency);
        }
    }
    fn visit(
        index: usize,
        edges: &[Vec<usize>],
        ids: &[&str],
        step: &str,
        states: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Result<()> {
        match states[index] {
            2 => return Ok(()),
            1 => {
                let start = stack
                    .iter()
                    .position(|node| *node == index)
                    .unwrap_or_default();
                let mut path: Vec<&str> = stack[start..].iter().map(|node| ids[*node]).collect();
                path.push(ids[index]);
                return Err(ActError::Model(format!(
                    "branch 'needs' in step '{step}' form a cycle: {}",
                    path.join(" -> ")
                )));
            }
            _ => {}
        }
        states[index] = 1;
        stack.push(index);
        for next in &edges[index] {
            visit(*next, edges, ids, step, states, stack)?;
        }
        stack.pop();
        states[index] = 2;
        Ok(())
    }
    let ids: Vec<&str> = step
        .branches
        .iter()
        .map(|branch| branch.id.as_str())
        .collect();
    let mut states = vec![0u8; step.branches.len()];
    let mut stack = Vec::new();
    for index in 0..step.branches.len() {
        visit(index, &edges, &ids, &step.id, &mut states, &mut stack)?;
    }
    Ok(())
}

/// The join rule only means something on a parallel container: a step whose
/// branches run together, or a `acts.core.block{ mode: parallel }` /
/// `acts.core.parallel` step whose children are the acts (or the per-item
/// templates) it fans out. Everywhere else it is refused — a rule on a
/// sequential shape would silently never fire.
///
/// The container packages carry the rule in their `params` (it lands on the
/// step when the import materializes their inline acts), so a malformed one is
/// parsed here and fails the start instead of the first join.
fn validate_join(step: &Step) -> Result<()> {
    let uses = step.uses.as_deref();
    let mode = step.params.get("mode").and_then(|mode| mode.as_str());
    let sequence_shape =
        uses == Some(SEQUENCE_PACKAGE) || (uses == Some(BLOCK_PACKAGE) && mode == Some("sequence"));
    let parallel_shape =
        (uses == Some(BLOCK_PACKAGE) && mode == Some("parallel")) || uses == Some(PARALLEL_PACKAGE);
    let declared_join = step.params.get("join").is_some_and(|join| !join.is_null());
    if declared_join {
        if sequence_shape {
            return Err(ActError::Model(format!(
                "step '{}' declares a join on a sequential container: its children run one after another, there is nothing to join",
                step.id
            )));
        }
        if parallel_shape {
            serde_json::from_value::<JoinRule>(step.params["join"].clone()).map_err(|error| {
                ActError::Model(format!("invalid join on step '{}': {error}", step.id))
            })?;
        }
    }
    let Some(join) = step.join.as_ref() else {
        return Ok(());
    };
    for rule in &step.rules {
        if rule.r#if.trim().is_empty() {
            return Err(ActError::Model(format!(
                "step '{}' declares a validation rule with an empty expression",
                step.id
            )));
        }
    }
    if step.next.is_some() {
        return Err(ActError::Model(format!(
            "step '{}' declares join '{join}' with an explicit 'next': the branches it would join are ignored by 'next'",
            step.id
        )));
    }
    if step.branches.is_empty() && !parallel_shape {
        return Err(ActError::Model(format!(
            "step '{}' declares join '{join}' but runs nothing in parallel: a join joins a container's branches",
            step.id
        )));
    }
    Ok(())
}

fn children_of(position: &Position) -> [Vec<String>; 3] {
    match &position.content {
        NodeContent::Workflow(model) => [
            model
                .steps
                .first()
                .map(|step| step.id.clone())
                .into_iter()
                .collect(),
            Vec::new(),
            Vec::new(),
        ],
        NodeContent::Step(step) => [
            if step.next.is_none() {
                step.branches
                    .iter()
                    .map(|branch| branch.id.clone())
                    .collect()
            } else {
                Vec::new()
            },
            step.catches
                .first()
                .map(|catch| catch.id.clone())
                .into_iter()
                .collect(),
            step.timeouts
                .first()
                .map(|timeout| timeout.id.clone())
                .into_iter()
                .collect(),
        ],
        NodeContent::Branch(branch) => [
            branch
                .steps
                .first()
                .map(|step| step.id.clone())
                .into_iter()
                .collect(),
            Vec::new(),
            Vec::new(),
        ],
    }
}

fn write_tree_output(
    model: &Workflow,
    id: &str,
    is_last: bool,
    parent_path: &HashMap<usize, bool>,
    visited: &mut HashSet<String>,
    output: &mut String,
) {
    let Some(position) = locate(model, id) else {
        return;
    };
    if !visited.insert(position.content.id()) {
        return;
    }

    let mut path = parent_path.clone();
    path.insert(position.level, !is_last);

    if position.level > 0 {
        for index in 1..position.level {
            if path.get(&index).copied().unwrap_or(false) {
                output.push_str("│   ");
            } else {
                output.push_str("    ");
            }
        }
        output.push_str(if is_last { "└── " } else { "├── " });
    }

    output.push_str(&format!(
        "{} id:{} name={}  next={}\n",
        position.content.kind(),
        position.content.id(),
        position.content.name(),
        position.next_id.as_deref().unwrap_or("nil")
    ));

    for children in children_of(&position) {
        let len = children.len();
        for (index, child) in children.iter().enumerate() {
            let mut child_is_last = index == len - 1;
            if let Some(next) = locate(model, child).and_then(|position| position.next_id)
                && !visited.contains(&next)
            {
                child_is_last = false;
            }
            write_tree_output(model, child, child_is_last, &path, visited, output);
        }
    }

    if let Some(next) = &position.next_id
        && !visited.contains(next)
    {
        let next_is_last = locate(model, next).is_none_or(|position| position.next_id.is_none());
        write_tree_output(model, next, next_is_last, &path, visited, output);
    }
}

/// The model-parse error with the failing line quoted and marked.
///
/// serde_yaml's message already carries the model path (`steps[2].assign.cc`)
/// and the line and column; the excerpt pins the spot in the source a model
/// author opened. Without the position — an error that surfaced before the
/// loader ever read an event — the message passes through unchanged.
fn locate_yaml_error(source: &str, error: &serde_yaml::Error) -> String {
    let Some(location) = error.location() else {
        return error.to_string();
    };
    let line = location.line();
    let column = location.column();
    let text = source.lines().nth(line.saturating_sub(1)).unwrap_or("");
    let caret = " ".repeat(column.saturating_sub(1));
    format!(
        "{error}\n{:>4} |\n{:>4} | {text}\n{:>4} | {caret}^",
        "", line, ""
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_tree_output_returns_for_unknown_and_visited_ids() {
        let model = Workflow::new().with_id("m1");
        let mut output = String::new();
        let mut visited = HashSet::new();
        let mut parent = HashMap::new();

        write_tree_output(&model, "missing", true, &parent, &mut visited, &mut output);
        assert!(output.is_empty());

        let mut path = HashMap::new();
        path.insert(0usize, false);
        write_tree_output(&model, "m1", true, &path, &mut visited, &mut output);
        assert!(output.contains("id:m1"), "{output}");
        let first_len = output.len();
        write_tree_output(&model, "m1", true, &parent, &mut visited, &mut output);
        assert_eq!(
            output.len(),
            first_len,
            "a visited node is not written twice"
        );
        parent.clear();
    }

    #[test]
    fn parse_error_quotes_and_marks_the_failing_line() {
        let text = "id: m1\nsteps:\n  - id: a\n    uses: acts.core.irq\n    assign:\n      cc:\n        params:\n          kind: owners_of\n          resource: { resource: department, name: finance }\n          duty: 3\n";
        let error = Workflow::from_yml(text).unwrap_err().to_string();
        // The exact failing value: the model path names the field, the
        // position is the `duty` scalar on line 10 — not the start of the
        // assign map (line 5) or of the expression object (line 8) — and the
        // excerpt quotes and marks the line.
        assert!(error.contains("steps[0].assign.cc.params.duty"), "{error}");
        assert!(error.contains("line 10 column 17"), "{error}");
        assert!(error.contains("duty: 3"), "{error}");
        assert!(error.contains("  10 | "), "{error}");
        assert!(error.contains('^'), "{error}");
    }

    #[test]
    fn parse_error_names_the_model_path_and_line_of_a_direct_field() {
        let text = "id: m1\nsteps:\n  - id: a\n    assign:\n      owner:\n        - alice\n";
        let error = Workflow::from_yml(text).unwrap_err().to_string();
        assert!(error.contains("steps[0].assign.owner"), "{error}");
        assert!(error.contains("line 6"), "{error}");
    }
}
