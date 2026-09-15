use crate::{MessageInfo, Principal, Result, query::Query, scheduler::Runtime, store::PageData};
use std::sync::Arc;
use tracing::{debug, instrument};

/// `msg:ls` — list delivery rows.
pub(crate) const LS: &str = "msg:ls";
/// `msg:get` — read one delivery row joined with its message.
pub(crate) const GET: &str = "msg:get";
/// `msg:ack` — acknowledge a delivery.
pub(crate) const ACK: &str = "msg:ack";
/// `msg:rm` — delete a delivery row.
pub(crate) const RM: &str = "msg:rm";
/// `msg:clear` — clear error deliveries, of one process or of all.
pub(crate) const CLEAR: &str = "msg:clear";
/// `msg:redo` — re-send error deliveries.
pub(crate) const REDO: &str = "msg:redo";
/// `msg:unsub` — unsubscribe a channel.
pub(crate) const UNSUB: &str = "msg:unsub";

#[derive(Clone)]
pub struct MessageExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl MessageExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
        }
    }

    /// List delivery rows. Every row is one delivery of a canonical message
    /// to one channel, identified by its delivery id (`id`); the shared
    /// message id is `msg_id` and the message payload is joined from the
    /// `messages` collection.
    #[instrument(skip(self))]
    pub async fn list(&self, q: &Query) -> Result<PageData<MessageInfo>> {
        self.principal.check(LS)?;
        match self.runtime.cache().store().deliveries().query(q).await {
            Ok(deliveries) => {
                let mut rows = Vec::with_capacity(deliveries.rows.len());
                for delivery in deliveries.rows.iter() {
                    if let Some(info) = self.delivery_info(delivery).await? {
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
    pub async fn get(&self, id: &str) -> Result<MessageInfo> {
        self.principal.check(GET)?;
        let delivery = &self.runtime.cache().store().deliveries().find(id).await?;
        match self.delivery_info(delivery).await? {
            Some(info) => Ok(info),
            None => Err(crate::ActError::Store(format!(
                "cannot find message for delivery '{}'",
                id
            ))),
        }
    }

    /// Join a delivery row with its canonical message.
    async fn delivery_info(&self, delivery: &crate::data::Delivery) -> Result<Option<MessageInfo>> {
        match self
            .runtime
            .cache()
            .store()
            .messages()
            .find(&delivery.msg_id)
            .await
        {
            Ok(message) => Ok(Some(MessageInfo::from_delivery(delivery, &message))),
            Err(err) => {
                debug!(delivery_id = %delivery.id, msg_id = %delivery.msg_id, error = %err, "orphan delivery row");
                Ok(None)
            }
        }
    }

    /// Ack one delivery row by its delivery id.
    pub async fn ack(&self, id: &str) -> Result<()> {
        self.principal.check(ACK)?;
        self.runtime.ack(id).await
    }

    /// Delete one delivery row by its delivery id.
    #[instrument(skip(self))]
    pub async fn rm(&self, id: &str) -> Result<bool> {
        self.principal.check(RM)?;
        self.runtime.cache().store().deliveries().delete(id).await
    }

    /// Clear error delivery rows: all of them (`None`) or only those of one
    /// process (`Some(pid)`).
    #[instrument(skip(self))]
    pub async fn clear(&self, pid: Option<String>) -> Result<()> {
        self.principal.check(CLEAR)?;
        self.runtime
            .cache()
            .store()
            .clear_error_deliveries(pid)
            .await?;
        Ok(())
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub async fn redo(&self) -> Result<()> {
        self.principal.check(REDO)?;
        self.runtime
            .cache()
            .store()
            .resend_error_deliveries()
            .await?;
        Ok(())
    }

    /// Delete one error delivery row by its delivery id.
    pub async fn clear_delivery(&self, delivery_id: &str) -> Result<()> {
        self.principal.check(CLEAR)?;
        self.runtime
            .cache()
            .store()
            .clear_error_delivery(delivery_id)
            .await?;
        Ok(())
    }

    /// Reset one error delivery row and immediately re-send it to the channel
    /// it belongs to.
    pub async fn redeliver(&self, delivery_id: &str) -> Result<()> {
        self.principal.check(REDO)?;
        if let Some(delivery) = self
            .runtime
            .cache()
            .store()
            .resend_error_delivery(delivery_id)
            .await?
        {
            match self
                .runtime
                .cache()
                .store()
                .messages()
                .find_opt(&delivery.msg_id)
                .await?
            {
                Some(message) => {
                    let mut msg: crate::Message = message.into();
                    msg.delivery_id = Some(delivery.id.clone());
                    self.runtime
                        .emitter()
                        .emit_delivery(&delivery.chan_id, &msg);
                }
                None => {
                    debug!(
                        delivery_id = %delivery.id,
                        msg_id = %delivery.msg_id,
                        "cannot re-send delivery: canonical message missing"
                    );
                }
            }
        }
        Ok(())
    }

    /// Unsubscribe a channel: no message is delivered to it any more.
    pub async fn unsub(&self, chan_id: &str) -> Result<()> {
        self.principal.check(UNSUB)?;
        self.runtime.emitter().remove(chan_id);
        Ok(())
    }
}
