mod catch;
mod retry;
mod timeout;

pub use catch::Catch;
pub use retry::Retry;

use crate::{ModelBase, StmtBuild, Vars};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[allow(unused_imports)]
pub use timeout::{Timeout, TimeoutLimit, TimeoutUnit};

use super::ActEvent;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Act {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    // to use a package, such as 'acts.transform.set'
    #[serde(default)]
    pub uses: String,

    // package params
    #[serde(default)]
    pub params: JsonValue,

    // package extra options
    // such as ACT_INDEX, ACT_VALUE
    #[serde(default)]
    pub options: Vars,

    #[serde(default)]
    pub r#if: Option<String>,

    /// act key for req and msg
    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub tag: String,

    /// on event for 'created', 'completed'
    #[serde(default)]
    pub on: Option<ActEvent>,

    /// act arguments
    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub setup: Vec<Act>,

    #[serde(default)]
    pub catches: Vec<Catch>,

    #[serde(default)]
    pub timeout: Vec<Timeout>,
}

impl ModelBase for Act {
    fn id(&self) -> &str {
        &self.id
    }
}

impl<T> StmtBuild<T> for Vec<T> {
    fn add(mut self, s: T) -> Self {
        self.push(s);
        self
    }

    fn with<F: Fn(T) -> T>(mut self, build: F) -> Self
    where
        T: Default,
    {
        self.push(build(T::default()));
        self
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

    pub fn with_key(mut self, key: &str) -> Self {
        self.key = key.to_string();
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.inputs.set(name, value);
        self
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

    #[cfg(test)]
    pub fn with_input_acts<T>(mut self, name: &str, f: fn(&mut Vec<T>)) -> Self
    where
        T: Serialize + Clone,
    {
        let mut vec = Vec::new();
        f(&mut vec);
        self.inputs.set(name, vec);
        self
    }

    pub fn with_output(mut self, name: &str, value: JsonValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_setup(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.setup = build(stmts);
        self
    }

    pub fn with_on(mut self, event: ActEvent) -> Self {
        self.on = Some(event);
        self
    }

    pub fn with_catch(mut self, build: fn(Catch) -> Catch) -> Self {
        let catch = Catch::default();
        self.catches.push(build(catch));
        self
    }

    pub fn with_timeout(mut self, build: fn(Timeout) -> Timeout) -> Self {
        let timeout = Timeout::default();
        self.timeout.push(build(timeout));
        self
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
}
