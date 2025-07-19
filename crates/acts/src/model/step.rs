use crate::utils::consts;
#[allow(unused_imports)]
use crate::{Act, Catch, ModelBase, Timeout, Vars, model::Branch};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub id: String,

    /// define the step vars
    #[serde(default)]
    pub vars: Vars,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub r#if: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub acts: Vec<Act>,

    #[serde(default)]
    pub catches: Vec<Act>,

    #[serde(default)]
    pub timeouts: Vec<Act>,

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

    pub fn with_act(mut self, act: Act) -> Self {
        self.acts.push(act);
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

    pub fn with_var<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.vars.set(name, value);
        self
    }

    pub fn with_output(mut self, name: &str, value: JsonValue) -> Self {
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

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
        self
    }

    pub fn with_catch(mut self, catch: Act) -> Self {
        self.catches.push(catch);
        self
    }

    pub fn with_timeout(mut self, timeout: Act) -> Self {
        self.timeouts.push(timeout);
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
