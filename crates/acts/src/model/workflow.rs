use crate::{Act, ActError, ModelBase, Result, Step, Vars, scheduler::NodeTree};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub env: Vars,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub setup: Vec<Act>,

    #[serde(default)]
    pub on: Vec<Act>,

    #[serde(default)]
    pub ver: i32,
}

impl Workflow {
    pub fn from_yml(s: &str) -> Result<Self> {
        let workflow = serde_yaml::from_str::<Workflow>(s);
        match workflow {
            Ok(v) => Ok(v),
            Err(e) => Err(ActError::Model(format!("{}", e))),
        }
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let workflow = serde_json::from_str::<Workflow>(s);
        match workflow {
            Ok(v) => Ok(v),
            Err(e) => Err(ActError::Model(format!("{}", e))),
        }
    }

    pub fn set_env(&mut self, vars: &Vars) {
        for (name, value) in vars {
            self.env
                .entry(name.clone())
                .and_modify(|v| *v = value.clone())
                .or_insert(value.clone());
        }
    }

    pub fn set_inputs(&mut self, vars: &Vars) {
        for (name, value) in vars {
            self.inputs
                .entry(name.clone())
                .and_modify(|v| *v = value.clone())
                .or_insert(value.clone());
        }
    }

    pub fn print(&self) {
        let mut root = NodeTree::new();
        root.load(self).unwrap();
        root.print();
    }

    pub fn tree_output(&self) -> String {
        let mut root = NodeTree::new();
        root.load(self).unwrap();
        root.tree_output()
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

    pub fn set_ver(&mut self, ver: i32) {
        self.ver = ver;
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

    pub fn valid(&self) -> Result<()> {
        let mut root = NodeTree::new();
        root.load(self)?;
        Ok(())
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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input(mut self, name: &str, value: JsonValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_env(mut self, name: &str, value: JsonValue) -> Self {
        self.env.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: JsonValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
        self
    }

    pub fn with_setup(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.setup = build(stmts);
        self
    }

    pub fn with_on(mut self, build: fn(Act) -> Act) -> Self {
        let act = build(Act::default());
        self.on.push(act);
        self
    }
}
