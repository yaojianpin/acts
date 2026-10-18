use crate::{Act, ActError, Branch, Result, Step, Variant, Vars, Workflow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeContent {
    Workflow(Arc<Workflow>),
    Branch(Branch),
    Step(Step),
    Act(Act),
}

#[derive(PartialEq, Default, Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind {
    #[default]
    Workflow,
    Branch,
    Step,
    Act,
}

/// The persisted shape of one model node — the `tasks.node_data` column.
///
/// It is derived straight from the declaring model (`Workflow`/`Step`/`Branch`)
/// and never carries a live graph: `parent_id`/`prev_id`/`next_id` are the
/// declaration links the row persists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    pub id: String,
    pub content: NodeContent,
    pub level: usize,
    /// parent node id, persisted for dynamic nodes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// previous node id in the dynamic chain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_id: Option<String>,
    /// next node id in the dynamic chain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_id: Option<String>,
}

impl NodeContent {
    pub fn id(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.id.clone(),
            NodeContent::Branch(data) => data.id.clone(),
            NodeContent::Step(data) => data.id.clone(),
            NodeContent::Act(data) => data.id.to_string(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.name.clone(),
            NodeContent::Branch(data) => data.name.clone(),
            NodeContent::Step(data) => data.name.clone(),
            NodeContent::Act(data) => data.name.to_string(),
        }
    }

    pub fn kind(&self) -> NodeKind {
        match self {
            NodeContent::Workflow(_) => NodeKind::Workflow,
            NodeContent::Branch(_) => NodeKind::Branch,
            NodeContent::Step(_) => NodeKind::Step,
            NodeContent::Act(_) => NodeKind::Act,
        }
    }

    pub fn vars(&self) -> Vars {
        match self {
            NodeContent::Workflow(data) => data.vars(),
            NodeContent::Branch(data) => data.vars(),
            NodeContent::Step(data) => data.vars(),
            NodeContent::Act(data) => data.vars(),
        }
    }

    pub fn options(&self) -> Vars {
        match self {
            NodeContent::Workflow(node) => node.options.clone(),
            NodeContent::Branch(node) => node.options.clone(),
            NodeContent::Step(node) => node.options.clone(),
            NodeContent::Act(node) => node.options.clone(),
        }
    }

    pub fn exposes(&self) -> &Vec<Variant> {
        match self {
            NodeContent::Workflow(node) => &node.exposes,
            NodeContent::Branch(node) => &node.exposes,
            NodeContent::Step(node) => &node.exposes,
            NodeContent::Act(node) => &node.exposes,
        }
    }

    pub fn params(&self) -> serde_json::Value {
        match self {
            NodeContent::Step(node) => node.params.clone(),
            NodeContent::Act(node) => node.params.clone(),
            _ => serde_json::Value::Null,
        }
    }
}

// The accessors here are the read-model test surface (`scheduler/shell/tests`,
// reliability matrix); the production row projection reads the fields.
#[cfg_attr(not(test), allow(dead_code))]
impl NodeData {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn kind(&self) -> NodeKind {
        self.content.kind()
    }

    /// The row's persisted JSON (`serde_json`, field order as declared).
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|err| ActError::Store(err.to_string()))
    }
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NodeKind::Workflow => "workflow",
            NodeKind::Branch => "branch",
            NodeKind::Step => "step",
            NodeKind::Act => "act",
        };
        f.write_str(s)
    }
}

impl From<NodeKind> for String {
    fn from(value: NodeKind) -> Self {
        value.to_string()
    }
}

impl From<String> for NodeKind {
    fn from(kind: String) -> Self {
        let s: &str = &kind;
        s.into()
    }
}

impl From<&str> for NodeKind {
    fn from(str: &str) -> Self {
        match str {
            "workflow" => NodeKind::Workflow,
            "branch" => NodeKind::Branch,
            "step" => NodeKind::Step,
            "act" => NodeKind::Act,
            _ => panic!("not found NodeKind: {str}"),
        }
    }
}
