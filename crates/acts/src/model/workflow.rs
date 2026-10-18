use crate::{
    ActError, ActSchema, Branch, ModelBase, Result, Step, Trigger, Variant, Vars,
    scheduler::{NodeContent, NodeData, assign_ids},
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
        let workflow = serde_yaml::from_str::<Workflow>(s);
        match workflow {
            Ok(v) => Ok(v),
            Err(e) => Err(ActError::Model(format!("{e}"))),
        }
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
            content: NodeContent::Step(step.clone()),
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
                content: NodeContent::Branch(branch.clone()),
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
        NodeContent::Act(_) => [Vec::new(), Vec::new(), Vec::new()],
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
