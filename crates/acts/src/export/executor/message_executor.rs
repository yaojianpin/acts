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
        let deliveries = self.runtime.store().deliveries().query(q).await?;
        // A delivery whose message row is gone is dropped from the page (its
        // metadata still counts it), so the rows are built before paging is
        // reattached.
        let mut rows = Vec::with_capacity(deliveries.rows.len());
        for delivery in deliveries.rows.iter() {
            if let Some(info) = self.delivery_info(delivery).await? {
                rows.push(info);
            }
        }
        Ok(deliveries.with_rows(rows))
    }

    /// Get one delivery row (joined with its message) by its delivery id.
    #[instrument(skip(self))]
    pub async fn get(&self, id: &str) -> Result<MessageInfo> {
        self.principal.check(GET)?;
        let delivery = &self.runtime.store().deliveries().find(id).await?;
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
        match self.runtime.store().messages().find(&delivery.msg_id).await {
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
        self.runtime.store().deliveries().delete(id).await
    }

    /// Clear error delivery rows: all of them (`None`) or only those of one
    /// process (`Some(pid)`).
    #[instrument(skip(self))]
    pub async fn clear(&self, pid: Option<String>) -> Result<()> {
        self.principal.check(CLEAR)?;
        self.runtime.store().clear_error_deliveries(pid).await?;
        Ok(())
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub async fn redo(&self) -> Result<()> {
        self.principal.check(REDO)?;
        self.runtime.store().resend_error_deliveries().await?;
        Ok(())
    }

    /// Delete one error delivery row by its delivery id.
    pub async fn clear_delivery(&self, delivery_id: &str) -> Result<()> {
        self.principal.check(CLEAR)?;
        self.runtime
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
            .store()
            .resend_error_delivery(delivery_id)
            .await?
        {
            match self
                .runtime
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Engine, Principal,
        data::{Delivery, DeliveryStatus, Message},
    };

    fn delivery(id: &str, msg_id: &str, status: DeliveryStatus) -> Delivery {
        Delivery {
            id: id.to_string(),
            msg_id: msg_id.to_string(),
            chan_id: "chan".to_string(),
            status,
            ..Default::default()
        }
    }

    fn message(id: &str) -> Message {
        Message {
            id: id.to_string(),
            r#type: "act".to_string(),
            ..Default::default()
        }
    }

    async fn executor(engine: &Engine) -> MessageExecutor {
        MessageExecutor::new(&engine.runtime(), &Arc::new(Principal::unrestricted()))
    }

    /// A delivery whose canonical message row is gone is dropped from the
    /// list (its metadata still counts it) and reported as an error by `get`.
    #[tokio::test]
    async fn an_orphan_delivery_is_dropped_from_the_list() {
        let engine = Engine::builder().start().await.unwrap();
        let store = engine.runtime().store();
        store
            .deliveries()
            .create(&delivery("d1", "missing", DeliveryStatus::Created))
            .await
            .unwrap();

        let executor = executor(&engine).await;
        let page = executor.list(&Query::new()).await.unwrap();
        assert!(page.rows.is_empty());
        assert_eq!(page.count, 1);

        let err = executor.get("d1").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot find message for delivery 'd1'"),
            "got: {err}"
        );
    }

    /// A joined delivery answers the message it carries.
    #[tokio::test]
    async fn a_delivery_joins_its_message() {
        let engine = Engine::builder().start().await.unwrap();
        let store = engine.runtime().store();
        store.messages().create(&message("m1")).await.unwrap();
        store
            .deliveries()
            .create(&delivery("d1", "m1", DeliveryStatus::Delivered))
            .await
            .unwrap();

        let executor = executor(&engine).await;
        let info = executor.get("d1").await.unwrap();
        assert_eq!(info.id, "d1");
        assert_eq!(executor.list(&Query::new()).await.unwrap().rows.len(), 1);
    }

    /// `redeliver` re-sends an error delivery, and silently does nothing when
    /// the canonical message is gone (the row is still reset for a later
    /// retry).
    #[tokio::test]
    async fn redeliver_resets_and_re_sends_an_error_delivery() {
        let engine = Engine::builder().start().await.unwrap();
        let store = engine.runtime().store();
        store.messages().create(&message("m1")).await.unwrap();
        store
            .deliveries()
            .create(&delivery("d1", "m1", DeliveryStatus::Error))
            .await
            .unwrap();
        // a second row whose message is missing exercises the drop path
        store
            .deliveries()
            .create(&delivery("d2", "gone", DeliveryStatus::Error))
            .await
            .unwrap();

        let executor = executor(&engine).await;
        executor.redeliver("d1").await.unwrap();
        executor.redeliver("d2").await.unwrap();
        // neither row exists at all: nothing to reset, nothing to send
        executor.redeliver("absent").await.unwrap();

        for id in ["d1", "d2"] {
            assert_eq!(
                store.deliveries().find(id).await.unwrap().status,
                DeliveryStatus::Created
            );
        }
    }

    /// Clearing one error delivery deletes exactly that row.
    #[tokio::test]
    async fn clearing_one_error_delivery_deletes_it() {
        let engine = Engine::builder().start().await.unwrap();
        let store = engine.runtime().store();
        store
            .deliveries()
            .create(&delivery("d1", "m1", DeliveryStatus::Error))
            .await
            .unwrap();

        let executor = executor(&engine).await;
        executor.clear_delivery("d1").await.unwrap();
        assert!(store.deliveries().find_opt("d1").await.unwrap().is_none());
        // an absent row is a no-op
        executor.clear_delivery("d1").await.unwrap();
    }
}
