use crate::{MessageInfo, Result, query::Query, scheduler::Runtime, store::PageData};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct MessageExecutor {
    runtime: Arc<Runtime>,
}

impl MessageExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }
    #[instrument(skip(self))]
    pub fn list(&self, q: &Query) -> Result<PageData<MessageInfo>> {
        match self.runtime.cache().store().messages().query(q) {
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

    pub fn ack(&self, id: &str) -> Result<()> {
        self.runtime.ack(id)
    }

    #[instrument(skip(self))]
    pub fn rm(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().messages().delete(id)
    }

    /// clear error messages
    pub fn clear(&self, pid: Option<String>) -> Result<()> {
        self.runtime.cache().store().clear_error_messages(pid)?;
        Ok(())
    }

    /// re-send error messages
    pub fn redo(&self) -> Result<()> {
        self.runtime.cache().store().resend_error_messages()?;
        Ok(())
    }

    /// unsubscribe the channel messages
    pub fn unsub(&self, chan_id: &str) -> Result<()> {
        self.runtime.emitter().remove(chan_id);
        Ok(())
    }
}
