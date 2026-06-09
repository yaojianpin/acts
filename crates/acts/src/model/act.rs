mod catch;
mod retry;
mod timeout;

pub use catch::Catch;
pub use retry::Retry;

use crate::{ModelBase, Variant, Vars, utils::consts};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

#[allow(unused_imports)]
pub use timeout::{Timeout, TimeoutLimit, TimeoutUnit};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Act {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub tag: String,

    // to use a package, such as 'acts.transform.set'
    #[serde(default)]
    pub uses: String,

    // package params
    #[serde(default)]
    pub params: JsonValue,

    #[serde(default)]
    pub r#if: Option<String>,

    /// act variables
    #[serde(default)]
    pub vars: Vec<Variant>,

    // package extra options to send to client
    // such as ACT_INDEX, ACT_VALUE
    #[serde(default)]
    pub options: Vars,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,
}

impl ModelBase for Act {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Act {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_uses(mut self, pack: &str) -> Self {
        self.uses = pack.to_string();
        self
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

    pub fn with_params_data(mut self, v: JsonValue) -> Self {
        self.params = v;
        self
    }

    pub fn with_params_vars<F: Fn(Vars) -> Vars>(mut self, build: F) -> Self {
        let vars = build(Vars::default());
        self.params = vars.into();

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

    pub fn with_if(mut self, v: &str) -> Self {
        self.r#if = Some(v.to_string());
        self
    }

    #[cfg(test)]
    pub fn set(params: Vars) -> Self {
        Act {
            params: params.into(),
            uses: "acts.transform.set".to_string(),
            ..Default::default()
        }
    }

    pub fn irq<T: Fn(Act) -> Act>(build: T) -> Self {
        let act = build(Act::default());
        Act {
            uses: "acts.core.irq".to_string(),
            ..act
        }
    }

    pub fn msg<T: Fn(Act) -> Act>(build: T) -> Self {
        let act = build(Act::default());
        Act {
            uses: "acts.core.msg".to_string(),
            ..act
        }
    }

    pub fn parallel(params: JsonValue) -> Self {
        Act {
            params,
            uses: "acts.core.parallel".to_string(),
            ..Default::default()
        }
    }

    pub fn subflow(params: JsonValue) -> Self {
        Act {
            params,
            uses: "acts.core.subflow".to_string(),
            ..Default::default()
        }
    }

    pub fn sequence(params: JsonValue) -> Self {
        Act {
            params,
            uses: "acts.core.sequence".to_string(),
            ..Default::default()
        }
    }

    pub fn action(params: Vars) -> Self {
        Act {
            params: params.into(),
            uses: "acts.core.action".to_string(),
            ..Default::default()
        }
    }

    pub fn block(params: Vars) -> Self {
        Act {
            params: params.into(),
            uses: "acts.core.block".to_string(),
            ..Default::default()
        }
    }

    pub fn code(code: &str) -> Self {
        Act {
            params: code.into(),
            uses: "acts.transform.code".to_string(),
            ..Default::default()
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
