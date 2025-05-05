use crate::{
    MessageInfo, Result, Vars,
    scheduler::Runtime,
    store::{PageData, StoreAdapter},
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
    pub fn list(&self, q: &ExecutorQuery) -> Result<PageData<MessageInfo>> {
        let query = q.into_query();
        match self.runtime.cache().store().messages().query(&query) {
            Ok(messages) => Ok(PageData {
                count: messages.count,
                page_size: messages.page_size,
                page_count: messages.page_count,
                page_num: messages.page_num,
                rows: messages.rows.iter().map(|m| m.into()).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, id: &str) -> Result<MessageInfo> {
        let message = &self.runtime.cache().store().messages().find(id)?;
        Ok(message.into())
    }

    pub fn do_event(&self, id: &str, options: &Vars) -> Result<()> {
        Ok(())
    }
}
