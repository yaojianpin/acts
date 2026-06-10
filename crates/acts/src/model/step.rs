#[allow(unused_imports)]
use crate::{Act, Catch, ModelBase, Timeout, Vars, model::Branch};
use crate::{Variant, utils::consts};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub id: String,

    /// resource name
    /// used in permission control
    pub rn: Option<String>,

    /// define the step vars
    #[serde(default)]
    pub vars: Vec<Variant>,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub r#if: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    // to use a package, such as 'acts.transform.set'
    #[serde(default)]
    pub uses: Option<String>,

    // package params
    #[serde(default)]
    pub params: JsonValue,

    #[serde(default)]
    pub catches: Vec<Step>,

    #[serde(default)]
    pub timeouts: Vec<Step>,

    /// extra options to send to client
    #[serde(default)]
    pub options: Vars,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,
}

impl ModelBase for Step {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Step {
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

    pub fn with_uses(mut self, uses: &str, params: Vars) -> Self {
        self.uses = Some(uses.to_string());
        self.params = params.into();
        self
    }

    pub fn with_uses_code(mut self, uses: &str, code: &str) -> Self {
        self.uses = Some(uses.to_string());
        self.params = code.into();
        self
    }

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    pub fn with_if(mut self, r#if: &str) -> Self {
        self.r#if = Some(r#if.to_string());
        self
    }

    pub fn with_rn(mut self, rn: &str) -> Self {
        self.rn = Some(rn.to_string());
        self
    }

    pub fn with_var<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.vars.push(Variant::create(name, json!(value)));
        self
    }

    pub fn vars(&self) -> Vars {
        let mut ret = Vars::new();
        self.vars.iter().for_each(|var| {
            ret.set(&var.name, var.value.clone());
        });

        ret
    }

    pub fn with_options<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.options.set(name, value);
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

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
        self
    }

    pub fn with_catch(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.catches.push(build(step));
        self
    }

    pub fn with_timeout(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.timeouts.push(build(step));
        self
    }

    pub fn with_metadata<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.metadata.set(name, value);
        self
    }
}
