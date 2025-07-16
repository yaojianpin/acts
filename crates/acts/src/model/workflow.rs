use crate::{Act, ActError, ModelBase, Result, Step, Vars, scheduler::NodeTree, utils::consts};
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

    /// define the workflow global vars
    #[serde(default)]
    pub vars: Vars,

    /// input json schema
    #[serde(default)]
    pub inputs: JsonValue,

    /// output json schema
    #[serde(default)]
    pub outputs: JsonValue,

    #[serde(default)]
    pub on: Vec<Act>,

    #[serde(default)]
    pub ver: i32,

    /// extra options to send to client
    #[serde(default)]
    pub options: Vars,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,
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

    pub fn set_env(&mut self, vars: &Vars) {
        for (name, value) in vars {
            self.env
                .entry(name.clone())
                .and_modify(|v| *v = value.clone())
                .or_insert(value.clone());
        }
    }

    pub fn set_vars(&mut self, vars: &Vars) {
        for (name, value) in vars {
            self.vars
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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_var(mut self, name: &str, value: JsonValue) -> Self {
        self.vars.insert(name.to_string(), value);
        self
    }

    pub fn with_env(mut self, name: &str, value: JsonValue) -> Self {
        self.env.insert(name.to_string(), value);
        self
    }

    pub fn with_inputs(mut self, inputs: JsonValue) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_outputs(mut self, outputs: JsonValue) -> Self {
        self.outputs = outputs;
        self
    }

    pub fn with_options_expose(mut self, name: &str, value: JsonValue) -> Self {
        self.options
            .entry(consts::ACT_EXPOSE)
            .and_modify(|outputs| {
                if let Some(obj) = outputs.as_object_mut() {
                    obj.insert(name.to_string(), value.clone());
                }
            })
            .or_insert(Vars::new().with(name, value).into());

        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
        self
    }

    pub fn with_on(mut self, build: fn(Act) -> Act) -> Self {
        let act = build(Act::default());
        self.on.push(act);
        self
    }
}
