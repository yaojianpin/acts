use crate::{Act, ActError, Candidate, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Chain {
    #[serde(default)]
    pub r#in: String,
    pub run: Vec<Act>,
}

impl Chain {
    pub fn parse(&self, ctx: &Context, scr: &str) -> Result<Candidate> {
        if scr.is_empty() {
            return Err(ActError::Runtime("chain's 'in' is empty".to_string()));
        }

        let result = ctx.eval_with::<rhai::Dynamic>(scr)?;
        let cand = Candidate::parse(&result.to_string())?;
        if cand.is_empty() {
            return Err(ActError::Runtime(format!(
                "chain.in is empty in task({})",
                ctx.task.id
            )));
        }
        Ok(cand)
    }

    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_in(mut self, scr: &str) -> Self {
        self.r#in = scr.to_string();
        self
    }

    pub fn with_run(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.run = build(stmts);
        self
    }
}
