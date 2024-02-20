mod catch;
mod timeout;

pub use self::{
    catch::Catch,
    timeout::{Timeout, TimeoutLimit, TimeoutUnit},
};
use crate::{model::Branch, Act, ActValue, ModelBase, Vars};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub run: Option<String>,

    #[serde(default)]
    pub r#if: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub acts: Vec<Act>,

    #[serde(default)]
    pub catches: Vec<Catch>,

    #[serde(default)]
    pub timeout: Vec<Timeout>,

    #[serde(default)]
    pub setup: Vec<Act>,
}

impl ModelBase for Step {
    fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("env", &self.inputs)
            .field("run", &self.run)
            .field("r#if", &self.r#if)
            .field("branches", &self.branches)
            .field("next", &self.next)
            .field("acts", &self.acts)
            .finish()
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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_act(mut self, stmt: Act) -> Self {
        self.acts.push(stmt);
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

    pub fn with_run(mut self, run: &str) -> Self {
        self.run = Some(run.to_string());
        self
    }

    pub fn with_input(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
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

    pub fn with_setup(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.setup = build(stmts);
        self
    }
}
