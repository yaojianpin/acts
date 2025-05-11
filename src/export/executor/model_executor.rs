use crate::{ModelInfo, Result, Workflow, scheduler::Runtime, store::PageData};
use std::sync::Arc;
use tracing::instrument;

use super::ExecutorQuery;

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

        // get the model triggers
        // for trigger in &model.on {}

        Ok(ret)
    }

    #[instrument(skip(self))]
    pub fn list(&self, q: &ExecutorQuery) -> Result<PageData<ModelInfo>> {
        let query = q.into_query();
        match self.runtime.cache().store().models().query(&query) {
            Ok(models) => Ok(PageData {
                count: models.count,
                page_size: models.page_size,
                page_count: models.page_count,
                page_num: models.page_num,
                rows: models.rows.iter().map(|m| m.into()).collect(),
            }),
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
