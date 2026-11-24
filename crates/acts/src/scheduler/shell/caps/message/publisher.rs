//! The durable half of outbox delivery: make the canonical message and the
//! registered ack channels' delivery rows exist before the outbox record may
//! be confirmed `Delivered`.
//!
//! The online emitter notification is a latency optimization on top of this.
//! Once this module returns a [`DurableMessageReceipt`], a crash at any later
//! instruction still leaves the message addressable, its content hash
//! checkable, and every ack channel's delivery responsibility recoverable by
//! the retry timer.

use crate::{
    Message, Result,
    event::DeliveryTargets,
    scheduler::core::outbox::records::{DurableMessageReceipt, OutboxReceipt},
    store::{Store, writebehind::StoreWriter},
};
use std::sync::Arc;

/// Why a durable handover did not complete.
#[derive(Debug)]
pub(crate) enum PublishError {
    /// The next layer could not be reached (store/writer failure). No external
    /// effect exists, so the record may safely be retried.
    Store(crate::ActError),
    /// The ids are already bound to different content, or a derived row is
    /// inconsistent. Retrying cannot repair it; the record is quarantined.
    Corrupt(String),
}

impl From<crate::ActError> for PublishError {
    fn from(error: crate::ActError) -> Self {
        Self::Store(error)
    }
}

/// Writes one message's durable facts and reports their addressable receipt.
pub(crate) struct DurableMessagePublisher {
    store: Arc<Store>,
    writer: StoreWriter,
    targets: Arc<DeliveryTargets>,
}

impl DurableMessagePublisher {
    pub(crate) fn new(
        store: Arc<Store>,
        writer: StoreWriter,
        targets: Arc<DeliveryTargets>,
    ) -> Self {
        Self {
            store,
            writer,
            targets,
        }
    }

    /// Persist `message` and every matching ack channel's delivery row.
    ///
    /// The message's window is taken from the durable task row, not from any
    /// forwarder-local observation: a restart cannot reset a run's start, and
    /// a terminal message after a restart carries the same duration it would
    /// have carried before one. The passed message is updated in place so the
    /// online notification carries exactly the stored facts.
    pub(crate) async fn publish(
        &self,
        message: &mut Message,
    ) -> std::result::Result<OutboxReceipt, PublishError> {
        self.hydrate_timing(message).await?;
        let row = message.clone().into_message();
        let targets = self.targets.matching(message);
        let mut delivery_ids = Vec::with_capacity(targets.len());
        for target in targets {
            let delivery = message.into_delivery(&target.chan_id, &target.pattern);
            match crate::store::writebehind::store_delivery(&self.writer, &row, &delivery).await {
                Ok(true) => delivery_ids.push(delivery.id),
                // The process's rows were already swept: there is no owner
                // left for a delivery responsibility. The canonical row is
                // still stored below, so the receipt remains addressable.
                Ok(false) => {}
                Err(error) => return Err(PublishError::Store(error)),
            }
        }
        if delivery_ids.is_empty() {
            self.store.store_canonical_message(&row).await?;
        }
        delivery_ids.sort_unstable();
        let delivery_hash = delivery_hash(&delivery_ids);
        let receipt = DurableMessageReceipt {
            message_id: row.id.clone(),
            payload_hash: row.content_hash(),
            delivery_count: u32::try_from(delivery_ids.len()).unwrap_or(u32::MAX),
            delivery_hash,
        };
        if !receipt.validate() {
            return Err(PublishError::Corrupt(
                "durable message receipt exceeds its bounds".into(),
            ));
        }
        Ok(OutboxReceipt::durable(receipt))
    }

    /// Fill the message's wall-clock window from the task row the projection
    /// already made durable. A message whose row is not there yet (a direct
    /// embedder emission, or a projection repaired later) falls back to the
    /// payload timestamp: it stays non-negative and deterministic across
    /// retries instead of becoming "now".
    async fn hydrate_timing(&self, message: &mut Message) -> Result<()> {
        let Some(task) = self
            .store
            .tasks()
            .find(&crate::utils::Id::new(&message.pid, &message.tid).id())
            .await?
        else {
            let fallback = message.timestamp.max(1);
            message.start_time = message.start_time.max(fallback);
            if message.state.is_completed() {
                message.end_time = message.end_time.max(message.start_time);
            }
            return Ok(());
        };
        let start = if task.start_time > 0 {
            task.start_time
        } else {
            message.timestamp.max(1)
        };
        message.start_time = start;
        if message.state.is_completed() {
            let end = if task.end_time > 0 {
                task.end_time
            } else {
                start
            };
            message.end_time = end.max(start);
        } else {
            message.end_time = 0;
        }
        Ok(())
    }
}

fn delivery_hash(ids: &[String]) -> u64 {
    let mut bytes = Vec::new();
    for id in ids {
        bytes.extend_from_slice(id.as_bytes());
        bytes.push(0);
    }
    xxhash_rust::xxh3::xxh3_64(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageState, Vars, event::DeliveryTarget, store::MemoryStore};

    fn message(id: &str) -> Message {
        Message {
            id: id.to_string(),
            tid: "t1".into(),
            pid: "p1".into(),
            nid: "s1".into(),
            mid: "m1".into(),
            r#type: "step".into(),
            state: MessageState::Created,
            inputs: Vars::new(),
            outputs: Vars::new(),
            timestamp: 10,
            ..Message::default()
        }
    }

    fn publisher(
        targets: Arc<DeliveryTargets>,
    ) -> (DurableMessagePublisher, Arc<Store>, StoreWriter) {
        let store = Arc::new(Store::new(Arc::new(MemoryStore::new())));
        let router = Arc::new(crate::utils::LaneRouter::new(1));
        let writer = StoreWriter::spawn(store.clone(), 1, 8, router);
        (
            DurableMessagePublisher::new(store.clone(), writer.clone(), targets),
            store,
            writer,
        )
    }

    /// A delivery row is only created for a live process; the publisher's
    /// tests that expect rows therefore seed the process row the projection
    /// would have written.
    async fn live_proc(store: &Store) {
        let now = crate::utils::time::time_millis();
        store
            .procs()
            .create(&crate::data::Proc {
                id: "p1".into(),
                state: "running".into(),
                mid: "m1".into(),
                name: "p1".into(),
                start_time: now,
                end_time: 0,
                timestamp: now,
                model: "{}".into(),
                env: "{}".into(),
                err: None,
                removable: false,
                source_revision: String::new(),
                v: 0,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn publish_is_idempotent_and_reports_the_stored_content() {
        let targets = Arc::new(DeliveryTargets::new());
        let (publisher, store, writer) = publisher(targets);
        let mut first = message("m1");
        let receipt = publisher.publish(&mut first).await.unwrap();
        let OutboxReceipt::DurableMessage(receipt) = receipt else {
            panic!("a durable handover must produce a durable receipt");
        };
        let stored = store.messages().find("m1").await.unwrap().unwrap();
        assert_eq!(receipt.payload_hash, stored.content_hash());
        assert_eq!(receipt.delivery_count, 0);

        let before = store.messages().find("m1").await.unwrap().unwrap();
        let mut again = message("m1");
        let second = publisher.publish(&mut again).await.unwrap();
        assert_eq!(second, OutboxReceipt::durable(receipt));
        assert_eq!(
            store
                .messages()
                .find("m1")
                .await
                .unwrap()
                .unwrap()
                .content_hash(),
            before.content_hash()
        );
        writer.close().await;
    }

    #[tokio::test]
    async fn publish_creates_one_deterministic_delivery_per_matching_ack_channel() {
        let targets = Arc::new(DeliveryTargets::new());
        targets.register(DeliveryTarget {
            chan_id: "alice/c1".into(),
            pattern: "{}".into(),
            ack: true,
            matches: Arc::new(|_| true),
        });
        targets.register(DeliveryTarget {
            chan_id: "bob/c2".into(),
            pattern: "{}".into(),
            ack: true,
            matches: Arc::new(|_| true),
        });
        targets.register(DeliveryTarget {
            chan_id: "bob/nonack".into(),
            pattern: "{}".into(),
            ack: false,
            matches: Arc::new(|_| true),
        });
        let (publisher, store, writer) = publisher(targets);
        live_proc(&store).await;
        let mut msg = message("m1");
        let OutboxReceipt::DurableMessage(receipt) = publisher.publish(&mut msg).await.unwrap()
        else {
            panic!("durable");
        };
        assert_eq!(receipt.delivery_count, 2);
        let expected_a = msg.into_delivery("alice/c1", "{}").id;
        let expected_b = msg.into_delivery("bob/c2", "{}").id;
        assert_eq!(
            store
                .deliveries()
                .find(&expected_a)
                .await
                .unwrap()
                .unwrap()
                .status,
            crate::data::DeliveryStatus::Created
        );
        assert_eq!(
            store
                .deliveries()
                .find(&expected_b)
                .await
                .unwrap()
                .unwrap()
                .status,
            crate::data::DeliveryStatus::Created
        );

        let mut again = message("m1");
        let OutboxReceipt::DurableMessage(second) = publisher.publish(&mut again).await.unwrap()
        else {
            panic!("durable");
        };
        assert_eq!(second, receipt, "a retry must converge on the same rows");
        writer.close().await;
    }

    #[tokio::test]
    async fn publish_rejects_same_id_different_content() {
        let targets = Arc::new(DeliveryTargets::new());
        let (publisher, _store, writer) = publisher(targets);
        let mut first = message("m1");
        publisher.publish(&mut first).await.unwrap();
        let mut different = message("m1");
        different.outputs.set("n", 1);
        let error = publisher.publish(&mut different).await.unwrap_err();
        assert!(matches!(error, PublishError::Store(_)), "{error:?}");
        writer.close().await;
    }

    #[tokio::test]
    async fn publish_uses_the_durable_task_window() {
        let store = Arc::new(Store::new(Arc::new(MemoryStore::new())));
        let task = crate::data::Task {
            id: "p1t1".into(),
            pid: "p1".into(),
            tid: "t1".into(),
            node_data: String::new(),
            kind: String::new(),
            prev: None,
            next: Vec::new(),
            parent: None,
            name: String::new(),
            state: "running".into(),
            err: None,
            start_time: 42,
            end_time: 0,
            timestamp: 0,
            source_revision: String::new(),
            v: 0,
        };
        store.tasks().create(&task).await.unwrap();
        let writer = StoreWriter::spawn(
            store.clone(),
            1,
            8,
            Arc::new(crate::utils::LaneRouter::new(1)),
        );
        let publisher = DurableMessagePublisher::new(
            store.clone(),
            writer.clone(),
            Arc::new(DeliveryTargets::new()),
        );
        let mut msg = message("m1");
        publisher.publish(&mut msg).await.unwrap();
        assert_eq!(msg.start_time, 42);
        writer.close().await;
    }

    #[tokio::test]
    async fn concurrent_publishers_converge_on_one_row() {
        let targets = Arc::new(DeliveryTargets::new());
        targets.register(DeliveryTarget {
            chan_id: "alice/c1".into(),
            pattern: "{}".into(),
            ack: true,
            matches: Arc::new(|_| true),
        });
        let (publisher, store, writer) = publisher(targets);
        live_proc(&store).await;
        let publisher = Arc::new(publisher);
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let publisher = publisher.clone();
            tasks.push(tokio::spawn(async move {
                let mut msg = message("m1");
                publisher.publish(&mut msg).await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let q = crate::query::Query::new().filter(
            crate::query::Filter::and().expr(crate::query::Expr::eq("msg_id", "m1".to_string())),
        );
        assert_eq!(store.deliveries().query(&q).await.unwrap().rows.len(), 1);
        writer.close().await;
    }
}
