use crate::{
    ActError, EventInfo, Result, Vars, scheduler::Runtime, store::PageData, utils::consts,
};
use std::sync::Arc;
use tracing::instrument;

use super::ExecutorQuery;

#[derive(Clone)]
pub struct EventExecutor {
    runtime: Arc<Runtime>,
}

impl EventExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self))]
    pub fn list(&self, q: &ExecutorQuery) -> Result<PageData<EventInfo>> {
        let query = q.into_query();
        match self.runtime.cache().store().events().query(&query) {
            Ok(events) => Ok(PageData {
                count: events.count,
                page_size: events.page_size,
                page_count: events.page_count,
                page_num: events.page_num,
                rows: events.rows.iter().map(|m| m.into()).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, id: &str) -> Result<EventInfo> {
        let event = &self.runtime.cache().store().events().find(id)?;
        Ok(event.into())
    }

    pub async fn start(&self, event_id: &str, params: &serde_json::Value) -> Result<Option<Vars>> {
        let event = self.runtime.cache().store().events().find(event_id)?;

        let register = self
            .runtime
            .package()
            .get(&event.uses)
            .ok_or(ActError::Runtime(format!(
                "cannot find the registed package '{}'",
                event.uses
            )))?;

        let options = Vars::new().with(consts::MODEL_ID, event.mid);

        let mut params = params.clone();
        if params.is_null() {
            params = serde_json::from_str(&event.params).map_err(|err| {
                ActError::Convert(format!("failed to deserialize params: {}", err))
            })?;
        }
        let package = (register.create)(params)?;
        let ret = package.start(&self.runtime, &options).await?;
        Ok(ret)
    }
}
