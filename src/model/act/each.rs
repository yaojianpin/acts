use crate::{Act, ActError, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Each {
    #[serde(default)]
    pub r#in: String,
    pub run: Vec<Act>,
}

impl Each {
    pub fn parse(&self, ctx: &Context, scr: &str) -> Result<Vec<String>> {
        if scr.is_empty() {
            return Err(ActError::Runtime("each's 'in' is empty".to_string()));
        }

        let result = ctx.eval::<Vec<String>>(scr)?;
        if result.len() == 0 {
            return Err(ActError::Runtime(format!(
                "each.in is empty in task({})",
                ctx.task().id
            )));
        }
        Ok(result)
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
