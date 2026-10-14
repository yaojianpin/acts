//! The delivery rows: one per (message x channel). The channel path writes
//! them through the write-behind queue, the retry timer re-sends the ones no
//! client acked, and the engine closes the ones its task owned when the task
//! finishes.
use crate::{
    Result,
    data::{self, DeliveryStatus},
    store::{Store, query::*},
    utils,
};

impl Store {
    /// Advance a stored delivery from `Created` to `Delivered` — the channel
    /// handler ran to completion, so the delivery succeeded. Only rows still
    /// `Created` move: a handler that acked (or was closed) while running
    /// must never be downgraded. The stored row is read under its document
    /// lock (`Store::update_delivery`), so a close or retry-pass write landing
    /// while the handler ran is seen — never overwritten by a stale `Created`.
    pub async fn mark_delivered(&self, id: &str) -> Result<()> {
        self.update_delivery(id, |mut delivery| {
            (delivery.status == DeliveryStatus::Created).then(|| {
                delivery.status = DeliveryStatus::Delivered;
                delivery
            })
        })
        .await?;
        Ok(())
    }

    /// Ack one delivery row (by its delivery id): set its status.
    pub async fn set_delivery(&self, id: &str, status: DeliveryStatus) -> Result<()> {
        let delivery = self
            .update_delivery(id, |mut delivery| {
                // `Completed` is the final state (the engine closed the
                // delivery) — a late ack must never downgrade it back to the
                // intermediate `Acked`. The stored status is read under the
                // row's lock, so an engine close that won the race is seen
                // here instead of being overwritten by a stale read.
                (delivery.status != DeliveryStatus::Completed).then(|| {
                    delivery.status = status;
                    delivery
                })
            })
            .await?;
        // it's ok there is no delivery, or it was already closed by the engine
        let Some(delivery) = delivery else {
            return Ok(());
        };
        // a delivery closed `Completed` by the engine may be the
        // process's last unsettled one — if the process is finished and
        // nothing is left unsettled, mark it removable for the sweeper.
        // `Acked` is only an intermediate state and never triggers the
        // mark. `Error` keeps the process alive for manual handling.
        if status == DeliveryStatus::Completed {
            let _ = self.try_mark_removable(&delivery.pid).await;
        }
        Ok(())
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub async fn resend_error_deliveries(&self) -> Result<()> {
        let collection = self.deliveries();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("status", DeliveryStatus::Error)));
        for mut delivery in collection.query_all(&q).await? {
            delivery.status = DeliveryStatus::Created;
            delivery.retry_times = 0;
            delivery.update_time = utils::time::time_millis();
            collection.update(&delivery).await?;
        }

        Ok(())
    }

    /// Delete error delivery rows: all of them or only those of one process.
    pub async fn clear_error_deliveries(&self, pid: Option<String>) -> Result<()> {
        let mut cond = Filter::and().expr(Expr::eq("status", DeliveryStatus::Error));
        if let Some(pid) = &pid {
            cond = cond.expr(Expr::eq("pid", pid));
        }

        // Exhaustive: an error row past a page limit must not survive as an
        // orphan that nothing ever retries or clears.
        self.deliveries().delete_all(Some(&cond)).await?;

        Ok(())
    }

    /// Reset one error delivery row back to `Created` for redelivery. Returns
    /// the delivery when it was an error delivery and was reset, `None`
    /// otherwise.
    pub async fn resend_error_delivery(&self, delivery_id: &str) -> Result<Option<data::Delivery>> {
        let collection = self.deliveries();
        let Some(mut delivery) = collection.find_opt(delivery_id).await? else {
            return Ok(None);
        };
        if delivery.status != DeliveryStatus::Error {
            return Ok(None);
        }

        delivery.status = DeliveryStatus::Created;
        delivery.retry_times = 0;
        delivery.update_time = utils::time::time_millis();
        if collection.update(&delivery).await? {
            Ok(Some(delivery))
        } else {
            Ok(None)
        }
    }

    /// Delete one error delivery row. Returns `true` when the row existed and
    /// was in error state and was deleted.
    pub async fn clear_error_delivery(&self, delivery_id: &str) -> Result<bool> {
        let collection = self.deliveries();
        let Some(delivery) = collection.find_opt(delivery_id).await? else {
            return Ok(false);
        };
        if delivery.status == DeliveryStatus::Error {
            collection.delete(delivery_id).await
        } else {
            Ok(false)
        }
    }
}
