use crate::{MessageInfo, Result, query::Query, scheduler::Runtime, store::PageData};
use std::sync::Arc;
use tracing::{debug, instrument};

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

    /// List delivery rows. Every row is one delivery of a canonical message
    /// to one channel, identified by its delivery id (`id`); the shared
    /// message id is `msg_id` and the message payload is joined from the
    /// `messages` collection.
    #[instrument(skip(self))]
    pub fn list(&self, q: &Query) -> Result<PageData<MessageInfo>> {
        match self.runtime.cache().store().deliveries().query(q) {
            Ok(deliveries) => {
                let mut rows = Vec::with_capacity(deliveries.rows.len());
                for delivery in deliveries.rows.iter() {
                    if let Some(info) = self.delivery_info(delivery)? {
                        rows.push(info);
                    }
                }
                Ok(PageData {
                    count: deliveries.count,
                    page_size: deliveries.page_size,
                    page_count: deliveries.page_count,
                    page_num: deliveries.page_num,
                    rows,
                })
            }
            Err(err) => Err(err),
        }
    }

    /// Get one delivery row (joined with its message) by its delivery id.
    #[instrument(skip(self))]
    pub fn get(&self, id: &str) -> Result<MessageInfo> {
        let delivery = &self.runtime.cache().store().deliveries().find(id)?;
        match self.delivery_info(delivery)? {
            Some(info) => Ok(info),
            None => Err(crate::ActError::Store(format!(
                "cannot find message for delivery '{}'",
                id
            ))),
        }
    }

    /// Join a delivery row with its canonical message.
    fn delivery_info(&self, delivery: &crate::data::Delivery) -> Result<Option<MessageInfo>> {
        match self
            .runtime
            .cache()
            .store()
            .messages()
            .find(&delivery.msg_id)
        {
            Ok(message) => Ok(Some(MessageInfo::from_delivery(delivery, &message))),
            Err(err) => {
                debug!(delivery_id = %delivery.id, msg_id = %delivery.msg_id, error = %err, "orphan delivery row");
                Ok(None)
            }
        }
    }

    /// Ack one delivery row by its delivery id.
    pub fn ack(&self, id: &str) -> Result<()> {
        self.runtime.ack(id)
    }

    /// Delete one delivery row by its delivery id.
    #[instrument(skip(self))]
    pub fn rm(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().deliveries().delete(id)
    }

    /// Clear error delivery rows: all of them (`None`) or only those of one
    /// process (`Some(pid)`).
    pub fn clear(&self, pid: Option<String>) -> Result<()> {
        self.runtime.cache().store().clear_error_deliveries(pid)?;
        Ok(())
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub fn redo(&self) -> Result<()> {
        self.runtime.cache().store().resend_error_deliveries()?;
        Ok(())
    }

    /// Delete one error delivery row by its delivery id.
    pub fn clear_delivery(&self, delivery_id: &str) -> Result<()> {
        self.runtime
            .cache()
            .store()
            .clear_error_delivery(delivery_id)?;
        Ok(())
    }

    /// Reset one error delivery row and immediately re-send it to the channel
    /// it belongs to.
    pub fn redeliver(&self, delivery_id: &str) -> Result<()> {
        if let Some(delivery) = self
            .runtime
            .cache()
            .store()
            .resend_error_delivery(delivery_id)?
        {
            if let Ok(message) = self
                .runtime
                .cache()
                .store()
                .messages()
                .find(&delivery.msg_id)
            {
                let mut msg: crate::Message = message.into();
                msg.delivery_id = Some(delivery.id.clone());
                self.runtime
                    .emitter()
                    .emit_delivery(&delivery.chan_id, &msg);
            } else {
                debug!(
                    delivery_id = %delivery.id,
                    msg_id = %delivery.msg_id,
                    "cannot re-send delivery: canonical message missing"
                );
            }
        }
        Ok(())
    }

    /// Unsubscribe a channel: no message is delivered to it any more.
    pub fn unsub(&self, chan_id: &str) -> Result<()> {
        self.runtime.emitter().remove(chan_id);
        Ok(())
    }
}
