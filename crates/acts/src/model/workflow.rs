use crate::{
    Act, ActError, ActSchema, ModelBase, Result, Step, Variant, Vars, scheduler::NodeTree,
    utils::consts,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default)]
    pub id: String,

    /// resource name
    /// the resource name is also used in permission control, the name seperated in ':'
    /// just likes 'acts:share:a:b:workflow_a'
    pub rn: Option<String>,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub env: Vec<Variant>,

    /// define the workflow global vars
    #[serde(default)]
    pub vars: Vec<Variant>,

    /// input json schema
    #[serde(default)]
    pub inputs: ActSchema,

    /// output json schema
    #[serde(default)]
    pub outputs: ActSchema,

    #[serde(default)]
    pub on: Vec<Act>,

    #[serde(default)]
    pub ver: String,

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
        for (ref name, value) in vars {
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

    pub fn with_rn(mut self, rn: &str) -> Self {
        self.rn = Some(rn.to_string());
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
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

    pub fn with_outputs(mut self, outputs: ActSchema) -> Self {
        self.outputs = outputs;
        self
    }

    pub fn with_expose<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.options
            .entry(consts::ACT_EXPOSE)
            .and_modify(|outputs| {
                if let Some(obj) = outputs.as_array_mut() {
                    obj.push(json!(Variant::create(name, value.clone())));
                }
            })
            .or_insert(json!(vec![Variant::create(name, value)]));

        self
    }

    pub fn exposes(&self) -> Vars {
        let mut vars = Vars::new();
        if let Some(exposes) = self.options.get::<Vec<Variant>>(consts::ACT_EXPOSE) {
            exposes.iter().for_each(|var| {
                vars.set(&var.name, var.value.clone());
            });
        }
        vars
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
