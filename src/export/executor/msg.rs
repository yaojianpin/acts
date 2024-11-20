use crate::{
    sch::Runtime,
    store::{Cond, Expr, StoreAdapter},
    MessageInfo, Query, Result,
};
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
    pub fn list(&self, pid: &str, count: usize) -> Result<Vec<MessageInfo>> {
        let query = Query::new()
            .push(Cond::and().push(Expr::eq("pid", pid.to_string())))
            .set_limit(10000);
        match self.runtime.cache().store().messages().query(&query) {
            Ok(mut messages) => {
                let mut ret = Vec::new();
                messages.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                for t in messages.iter().take(count) {
                    ret.push(t.into());
                }

                Ok(ret)
            }
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
    pub fn clear(&self) -> Result<()> {
        self.runtime.cache().store().clear_error_messages()?;
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
