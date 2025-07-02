use crate::{ModelBase, Vars, model::Step, utils::consts};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Branch {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub vars: Vars,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub run: Option<String>,

    pub r#if: Option<String>,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub r#else: bool,

    #[serde(default)]
    pub needs: Vec<String>,

    /// extra options to send to client
    #[serde(default)]
    pub options: Vars,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,
}

impl ModelBase for Branch {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Branch {
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

    pub fn with_var(mut self, name: &str, value: JsonValue) -> Self {
        self.vars.insert(name.to_string(), value);
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

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    pub fn with_else(mut self, default: bool) -> Self {
        self.r#else = default;
        self
    }

    pub fn with_if(mut self, r#if: &str) -> Self {
        self.r#if = Some(r#if.to_string());
        self
    }

    pub fn with_run(mut self, run: &str) -> Self {
        self.run = Some(run.to_string());
        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
        self
    }

    pub fn with_need(mut self, need: &str) -> Self {
        self.needs.push(need.to_string());
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
