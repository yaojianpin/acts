//! The message-retry timer: re-sends unacknowledged messages, each
//! not-yet-acked delivery row to the channel it belongs to only. The loop
//! itself is the kernel host's serve tick ([`TimerGuider::retry_tick`]).

use tracing::error;

use super::TimerGuider;
use crate::ActError;

impl TimerGuider {
    /// One message-retry pass: every not-yet-acked delivery row is re-sent to
    /// the channel it belongs to only. One pass is one attempt: every store
    /// call below runs, and the pass counts as failed if any of them did — a
    /// store that cannot serve one of them cannot serve the next tick either.
    pub(crate) async fn retry_tick(&self) {
        // A degraded loop sits this tick out: the store has failed every
        // recent attempt, so another query now would fail too (and log
        // again) — the guard decides when the next attempt is worth making.
        if !self.timers.retry().attempt() {
            return;
        }
        let max_message_retry_times = self.runtime.config().max_message_retry_times();
        // The delivery age window is the loop's own interval: a row younger
        // than one interval has not waited out a tick yet.
        let config = self.runtime.config();
        let interval_ms = super::loop_interval(
            config.data.retry_interval_secs,
            config.retry_interval_secs(),
            super::tick_interval(config),
        )
        .as_millis() as i64;
        let evt = self.runtime.emitter().clone();
        let store = self.runtime.store();
        let mut failure: Option<ActError> = None;

        match store
            .with_no_response_deliveries(interval_ms, max_message_retry_times)
            .await
        {
            Ok(rearmed) => {
                for d in rearmed {
                    match store.messages().find(&d.msg_id).await {
                        Ok(message) => {
                            let mut msg: crate::event::Message = message.into();
                            msg.delivery_id = Some(d.id.clone());
                            evt.emit_delivery(&d.chan_id, &msg);
                        }
                        Err(err) => {
                            // orphan delivery: its canonical message is gone,
                            // it can never be re-sent — drop it
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
            Some(err) => self.timers.retry().failed(&err),
            None => self.timers.retry().recovered(),
        }
    }
}
