use crate::{
    Act, ModelInfo, Result, Workflow, data,
    query::{Cond, Expr, Query},
    scheduler::Runtime,
    store::PageData,
    utils::consts,
};
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

        let store = self.runtime.cache().store();
        let ret = store.deploy(model)?;
        self.deploy_event(&model.on, &model.id, model.ver)?;

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
        let store = self.runtime.cache().store();

        // find the model events and delete them
        let events = store
            .events()
            .query(&Query::new().push(Cond::and().push(Expr::eq(consts::MODEL_ID, id))))?;
        for evt in events.rows {
            store.events().delete(&evt.id)?;
        }

        // remove the model
        store.models().delete(id)
    }

    fn deploy_event(&self, acts: &[Act], mid: &str, ver: i32) -> Result<()> {
        let store = self.runtime.cache().store();
        for act in acts {
            let event_id = format!("{}:{}", mid, act.id);
            match store.events().find(&event_id) {
                Ok(evt) => {
                    if evt.ver == ver {
                        continue;
                    }
                    store
                        .events()
                        .update(&data::Event::from_act(act, mid, ver, &event_id)?)?;
                }
                Err(_) => {
                    store
                        .events()
                        .create(&data::Event::from_act(act, mid, ver, &event_id)?)?;
                }
            }
        }

        Ok(())
    }
}
