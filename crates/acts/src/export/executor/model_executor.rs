use crate::{
    ModelInfo, Principal, Result, Workflow, query::Query, scheduler::Runtime, store::PageData,
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tracing::instrument;

/// `model:ls` — list deployed models.
pub(crate) const LS: &str = "model:ls";
/// `model:get` — read one deployed model.
pub(crate) const GET: &str = "model:get";
/// `model:deploy` — write a model (and its triggers) into the catalogue.
pub(crate) const DEPLOY: &str = "model:deploy";
/// `model:rm` — delete a model and its triggers.
pub(crate) const RM: &str = "model:rm";

#[derive(Clone)]
pub struct ModelExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl ModelExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
        }
    }

    #[instrument(skip(self, model, view), fields(id = %model.id, name = %model.name))]
    pub async fn deploy(&self, model: &Workflow, view: Option<&JsonValue>) -> Result<bool> {
        self.principal.check(DEPLOY)?;
        // The resource the model claims must be within the caller's grants:
        // a model without an `rn` is only deployable by an unrestricted
        // user.
        self.principal.check_rn(&model.rn)?;
        model.valid()?;

        // The model row and its trigger (`events`) rows are reconciled and
        // committed as one atomic batch inside `Store::deploy`.
        self.runtime.cache().store().deploy(model, view).await
    }

    #[instrument(skip(self, q))]
    pub async fn list(&self, q: &Query) -> Result<PageData<ModelInfo>> {
        self.principal.check(LS)?;
        match self.runtime.cache().store().models().query(q).await {
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

    #[instrument(skip(self), fields(id = %id))]
    pub async fn get(&self, id: &str, fmt: &str) -> Result<ModelInfo> {
        self.principal.check(GET)?;
        match self.runtime.cache().store().models().find(id).await {
            Ok(m) => {
                let mut model: ModelInfo = m.into();
                match fmt {
                    "tree" => {
                        let workflow = Workflow::from_yml(&model.data)?;
                        model.data = workflow.tree_output();
                    }
                    "yaml" => {
                        let workflow = Workflow::from_yml(&model.data)?;
                        model.data = workflow.to_yml()?;
                    }
                    _ => {
                        let workflow = Workflow::from_yml(&model.data)?;
                        model.data = workflow.to_json()?;
                    }
                }

                Ok(model)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self), fields(id = %id))]
    pub async fn rm(&self, id: &str) -> Result<bool> {
        self.principal.check(RM)?;
        // The model row and its trigger (`events`) rows are removed as one
        // atomic batch inside `Store::rm_model`.
        self.runtime.cache().store().rm_model(id).await
    }
}
