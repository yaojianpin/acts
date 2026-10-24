//! The message-retry timer: periodically re-sends unacknowledged messages.
//!
//! The delivery rows are written through the channel path (which the kernel's
//! outbox transport feeds) and the rows themselves are the kernel's
//! projection, so this store-facing loop runs here rather than inside the
//! driver — it is not part of the operation protocol and must not delay it.
//! Terminal-run cleanup is a separate loop ([`TimerGuider::cleanup_timer`])
//! with its own budget and health.

use std::time::Duration;

use tokio::{runtime::Handle, time};
use tracing::error;

use super::TimerGuider;
use crate::ActError;

impl TimerGuider {
    /// Message-retry timer — periodically re-sends unacknowledged messages,
    /// each not-yet-acked delivery row to the channel it belongs to only.
    pub(crate) fn retry_timer(&self) -> crate::Result<()> {
        let max_message_retry_times = self.runtime.config().max_message_retry_times();
        // A10: the delivery retry cadence is its own; moving it never moves
        // the trigger, snapshot or driver loops.
        let config = self.runtime.config();
        let interval_ms = super::loop_interval(
            config.data.retry_interval_secs,
            config.retry_interval_secs(),
            super::tick_interval(config),
        )
        .as_millis() as u64;

        let evt = self.runtime.emitter().clone();
        let store = self.runtime.store();
        let shutdown = self.runtime.shutdown_token();
        let timers = self.timers.clone();
        Handle::current().spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _= shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                // A degraded loop sits this tick out: the store has failed
                // every recent attempt, so another query now would fail too
                // (and log again) — the guard decides when the next attempt is
                // worth making.
                if !timers.retry().attempt() {
                    continue;
                }
                // One tick is one attempt: every store call below runs, and the
                // tick counts as failed if any of them did — a store that
                // cannot serve one of them cannot serve the next tick either.
                let mut failure: Option<ActError> = None;

                // each not-yet-acked delivery row is re-sent to the channel it
                // belongs to only
                match store
                    .with_no_response_deliveries(interval_ms as i64, max_message_retry_times)
                    .await
                {
                    Ok(rearmed) => {
                        for d in rearmed {
                            let store = store.clone();
                            match store.messages().find(&d.msg_id).await {
                                Ok(message) => {
                                    let emitter = evt.clone();
                                    let mut msg: crate::event::Message = message.into();
                                    msg.delivery_id = Some(d.id.clone());
                                    emitter.emit_delivery(&d.chan_id, &msg);
                                }
                                Err(err) => {
                                    // orphan delivery: its canonical message
                                    // is gone, it can never be re-sent — drop it
                                    error!(delivery_id = %d.id, msg_id = %d.msg_id, error = %err, "delivery without canonical message dropped");
                                    if let Err(e) = store.deliveries().delete(&d.id).await {
                                        error!(error = %e, "orphan delivery delete failed");
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        error!(error = %err, "no-response deliveries query failed");
                        failure = Some(err);
                    }
                }

                match failure {
                    Some(err) => timers.retry().failed(&err),
                    None => timers.retry().recovered(),
                }
            }
        });

        Ok(())
    }
}
