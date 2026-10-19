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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    fn store() -> Store {
        Store::new(std::sync::Arc::new(MemoryStore::new()))
    }

    fn delivery(id: &str, pid: &str, status: DeliveryStatus) -> data::Delivery {
        data::Delivery {
            id: id.to_string(),
            msg_id: format!("msg-{id}"),
            pid: pid.to_string(),
            status,
            retry_times: 2,
            ..Default::default()
        }
    }

    async fn seed(store: &Store, row: data::Delivery) {
        store.deliveries().create(&row).await.unwrap();
    }

    async fn find(store: &Store, id: &str) -> data::Delivery {
        store.deliveries().find(id).await.unwrap()
    }

    #[tokio::test]
    async fn set_delivery_ignores_absent_rows_and_never_downgrades_completed() {
        let store = store();
        // Absent row: the ack is a no-op, not an error.
        store
            .set_delivery("missing", DeliveryStatus::Acked)
            .await
            .unwrap();

        seed(&store, delivery("d1", "p1", DeliveryStatus::Completed)).await;
        // A late ack must not downgrade the engine's final state.
        store
            .set_delivery("d1", DeliveryStatus::Acked)
            .await
            .unwrap();
        assert_eq!(find(&store, "d1").await.status, DeliveryStatus::Completed);

        seed(&store, delivery("d2", "p1", DeliveryStatus::Created)).await;
        store
            .set_delivery("d2", DeliveryStatus::Acked)
            .await
            .unwrap();
        assert_eq!(find(&store, "d2").await.status, DeliveryStatus::Acked);
    }

    #[tokio::test]
    async fn set_delivery_completed_marks_a_finished_process_removable() {
        let store = store();
        store
            .procs()
            .create(&data::Proc {
                id: "p1".into(),
                state: "completed".into(),
                mid: String::new(),
                name: String::new(),
                start_time: 0,
                end_time: 0,
                timestamp: 0,
                model: String::new(),
                env: String::new(),
                err: None,
                removable: false,
                source_revision: String::new(),
                v: 0,
            })
            .await
            .unwrap();
        seed(&store, delivery("d1", "p1", DeliveryStatus::Delivered)).await;

        store
            .set_delivery("d1", DeliveryStatus::Completed)
            .await
            .unwrap();
        assert_eq!(find(&store, "d1").await.status, DeliveryStatus::Completed);
        assert!(
            store.procs().find("p1").await.unwrap().removable,
            "the last settled delivery must mark a finished process removable"
        );
    }

    #[tokio::test]
    async fn mark_delivered_only_moves_created_rows() {
        let store = store();
        seed(&store, delivery("d1", "p1", DeliveryStatus::Created)).await;
        store.mark_delivered("d1").await.unwrap();
        assert_eq!(find(&store, "d1").await.status, DeliveryStatus::Delivered);

        seed(&store, delivery("d2", "p1", DeliveryStatus::Acked)).await;
        store.mark_delivered("d2").await.unwrap();
        assert_eq!(find(&store, "d2").await.status, DeliveryStatus::Acked);
    }

    #[tokio::test]
    async fn resend_and_clear_error_deliveries_by_pid_or_all() {
        let store = store();
        seed(&store, delivery("e1", "p1", DeliveryStatus::Error)).await;
        seed(&store, delivery("e2", "p2", DeliveryStatus::Error)).await;
        seed(&store, delivery("ok", "p1", DeliveryStatus::Acked)).await;

        store.resend_error_deliveries().await.unwrap();
        for id in ["e1", "e2"] {
            let row = find(&store, id).await;
            assert_eq!(row.status, DeliveryStatus::Created);
            assert_eq!(row.retry_times, 0);
        }
        assert_eq!(
            find(&store, "ok").await.status,
            DeliveryStatus::Acked,
            "a non-error row is untouched"
        );

        // Put the rows back into the error state and clear only one
        // process's error rows.
        store
            .set_delivery("e1", DeliveryStatus::Error)
            .await
            .unwrap();
        store
            .set_delivery("e2", DeliveryStatus::Error)
            .await
            .unwrap();
        store
            .clear_error_deliveries(Some("p1".to_string()))
            .await
            .unwrap();
        assert!(store.deliveries().find_opt("e1").await.unwrap().is_none());
        assert_eq!(
            store.deliveries().find("e2").await.unwrap().status,
            DeliveryStatus::Error,
            "another process's error row is kept"
        );

        // ... and then all of them.
        store.clear_error_deliveries(None).await.unwrap();
        assert!(store.deliveries().find_opt("e2").await.unwrap().is_none());
        assert!(store.deliveries().find_opt("ok").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn resend_error_delivery_handles_missing_non_error_and_error_rows() {
        let store = store();
        assert!(
            store
                .resend_error_delivery("missing")
                .await
                .unwrap()
                .is_none()
        );

        seed(&store, delivery("ok", "p1", DeliveryStatus::Delivered)).await;
        assert!(
            store.resend_error_delivery("ok").await.unwrap().is_none(),
            "only error rows are reset"
        );

        seed(&store, delivery("e1", "p1", DeliveryStatus::Error)).await;
        let reset = store.resend_error_delivery("e1").await.unwrap().unwrap();
        assert_eq!(reset.status, DeliveryStatus::Created);
        assert_eq!(reset.retry_times, 0);
        assert_eq!(find(&store, "e1").await.status, DeliveryStatus::Created);
    }

    #[tokio::test]
    async fn clear_error_delivery_handles_missing_non_error_and_error_rows() {
        let store = store();
        assert!(!store.clear_error_delivery("missing").await.unwrap());

        seed(&store, delivery("ok", "p1", DeliveryStatus::Acked)).await;
        assert!(!store.clear_error_delivery("ok").await.unwrap());
        assert!(store.deliveries().find_opt("ok").await.unwrap().is_some());

        seed(&store, delivery("e1", "p1", DeliveryStatus::Error)).await;
        assert!(store.clear_error_delivery("e1").await.unwrap());
        assert!(store.deliveries().find_opt("e1").await.unwrap().is_none());
    }
}
