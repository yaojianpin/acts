use crate::{ActError, Branch, Result, Step, Variant, Vars, Workflow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeContent {
    Workflow(Arc<Workflow>),
    Branch(Branch),
    Step(Step),
}

/// The word a node speaks where a kind is asked for. `workflow|branch|step`
/// are the declaration's levels (the rows' and the tree renderings' words),
/// `act` is the legacy payload level, and `block` is the kernel's own virtual
/// container — the branch the import materializes for a container package's
/// inline acts, which no declaration declares.
#[derive(PartialEq, Default, Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind {
    #[default]
    Workflow,
    Branch,
    Step,
    Act,
    Block,
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
        }
    }

    pub fn name(&self) -> String {
        match self {
            NodeContent::Workflow(data) => data.name.clone(),
            NodeContent::Branch(data) => data.name.clone(),
            NodeContent::Step(data) => data.name.clone(),
        }
    }

    pub fn kind(&self) -> NodeKind {
        match self {
            NodeContent::Workflow(_) => NodeKind::Workflow,
            NodeContent::Branch(_) => NodeKind::Branch,
            NodeContent::Step(_) => NodeKind::Step,
        }
    }

    pub fn vars(&self) -> Vars {
        match self {
            NodeContent::Workflow(data) => data.vars(),
            NodeContent::Branch(data) => data.vars(),
            NodeContent::Step(data) => data.vars(),
        }
    }

    pub fn options(&self) -> Vars {
        match self {
            NodeContent::Workflow(node) => node.options.clone(),
            NodeContent::Branch(node) => node.options.clone(),
            NodeContent::Step(node) => node.options.clone(),
        }
    }

    pub fn exposes(&self) -> &Vec<Variant> {
        match self {
            NodeContent::Workflow(node) => &node.exposes,
            NodeContent::Branch(node) => &node.exposes,
            NodeContent::Step(node) => &node.exposes,
        }
    }

    pub fn params(&self) -> serde_json::Value {
        match self {
            NodeContent::Step(node) => node.params.clone(),
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
            NodeKind::Block => "block",
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
            "block" => NodeKind::Block,
            _ => panic!("not found NodeKind: {str}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow() -> Workflow {
        let mut workflow = Workflow::new();
        workflow.id = "wf".into();
        workflow.name = "Wf".into();
        workflow
    }

    fn contents() -> Vec<NodeContent> {
        vec![
            NodeContent::Workflow(Arc::new(workflow())),
            NodeContent::Branch(Branch::new().with_id("br").with_name("Br")),
            NodeContent::Step(Step::new().with_id("st").with_name("St")),
        ]
    }

    /// Every content variant answers the same question from its own fields;
    /// the branch/step arms are exercised so a variant cannot inherit the
    /// wrong id/kind by accident.
    #[test]
    fn content_accessors_read_the_declaring_node() {
        let contents = contents();
        assert_eq!(contents[0].id(), "wf");
        assert_eq!(contents[1].id(), "br");
        assert_eq!(contents[2].id(), "st");
        assert_eq!(contents[0].name(), "Wf");
        assert_eq!(contents[1].name(), "Br");
        assert_eq!(contents[2].name(), "St");
        assert_eq!(contents[0].kind(), NodeKind::Workflow);
        assert_eq!(contents[1].kind(), NodeKind::Branch);
        assert_eq!(contents[2].kind(), NodeKind::Step);

        for content in &contents {
            assert!(content.vars().is_empty());
            assert!(content.options().is_empty());
            assert!(content.exposes().is_empty());
        }
        // Only a step declares params; a container has none.
        assert_eq!(contents[0].params(), serde_json::Value::Null);
        assert_eq!(contents[1].params(), serde_json::Value::Null);
        assert_eq!(contents[2].params(), serde_json::Value::Null);
    }

    #[test]
    fn a_step_params_are_exposed_verbatim() {
        let step = Step::new()
            .with_id("st")
            .with_uses("pack", Vars::new().with("a", 1));
        let content = NodeContent::Step(step);
        assert!(content.params().is_object());
        assert_eq!(content.params()["a"], serde_json::json!(1));
    }

    #[test]
    fn node_data_renders_its_persisted_json() {
        let data = NodeData {
            id: "st".into(),
            content: NodeContent::Step(Step::new().with_id("st").with_name("St")),
            level: 2,
            parent_id: Some("parent".into()),
            prev_id: None,
            next_id: Some("next".into()),
        };
        assert_eq!(data.id(), "st");
        assert_eq!(data.kind(), NodeKind::Step);
        let json = data.to_string().unwrap();
        assert!(json.contains("\"id\":\"st\""));
        assert!(json.contains("\"level\":2"));
        assert!(json.contains("\"parent_id\":\"parent\""));
        assert!(json.contains("\"next_id\":\"next\""));
        assert!(!json.contains("prev_id"), "an absent link is not written");
    }

    #[test]
    fn node_kind_converts_both_ways() {
        for (kind, name) in [
            (NodeKind::Workflow, "workflow"),
            (NodeKind::Branch, "branch"),
            (NodeKind::Step, "step"),
            (NodeKind::Act, "act"),
            (NodeKind::Block, "block"),
        ] {
            assert_eq!(kind.to_string(), name);
            assert_eq!(String::from(kind.clone()), name);
            assert_eq!(NodeKind::from(name.to_string()), kind);
            assert_eq!(NodeKind::from(name), kind);
        }
        assert_eq!(NodeKind::default(), NodeKind::Workflow);
    }

    #[test]
    #[should_panic(expected = "not found NodeKind: bogus")]
    fn an_unknown_node_kind_panics() {
        let _ = NodeKind::from("bogus");
    }
}
