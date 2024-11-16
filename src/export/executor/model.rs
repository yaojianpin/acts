use crate::{sch::Runtime, store::StoreAdapter, ModelInfo, Query, Result, Workflow};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct ModelExecutor {
    runtime: Arc<Runtime>,
}

impl ModelExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self))]
    pub fn deploy(&self, model: &Workflow) -> Result<bool> {
        model.valid()?;
        let ret = self.runtime.cache().store().deploy(model)?;
        Ok(ret)
    }

    #[instrument(skip(self))]
    pub fn list(&self, limit: usize) -> Result<Vec<ModelInfo>> {
        let query = Query::new().set_limit(limit);
        match self.runtime.cache().store().models().query(&query) {
            Ok(models) => {
                let mut ret = Vec::new();
                for m in models {
                    ret.push(m.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, id: &str, fmt: &str) -> Result<ModelInfo> {
        match self.runtime.cache().store().models().find(id) {
            Ok(m) => {
                let mut model: ModelInfo = m.into();
                if fmt == "tree" {
                    let workflow = Workflow::from_yml(&model.data)?;
                    model.data = workflow.tree_output();
                }
                Ok(model)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn rm(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().models().delete(id)
    }
}
