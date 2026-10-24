//! The schedule-trigger timer: fires every due `schedule` trigger row and
//! rolls its `next_run` forward. Deployed rows arm with their next cron fire;
//! a changed schedule re-arms the same way. The loop itself is the kernel
//! host's serve tick ([`TimerGuider::trigger_tick`]); this file holds the
//! cadence-independent pass and the fire primitive.

use std::sync::Arc;

use tracing::error;

use super::TimerGuider;
use crate::scheduler::Runtime;

use crate::{ActError, Result, Vars, data};

/// Fire one due schedule trigger: start the workflow with the trigger's
/// default params and roll `last_run`/`next_run` forward. The row state is
/// persisted after the start, so a crash between start and state roll may
/// re-fire the trigger on recovery (at-least-once).
pub(crate) async fn fire_schedule(runtime: &Arc<Runtime>, event: &data::Event) -> Result<()> {
    let model = runtime.store().models().find(&event.mid).await?;
    let model: crate::ModelInfo = model.into();
    let workflow = model.workflow()?;

    let payload = event.default_params();
    let inputs = match payload {
        serde_json::Value::Null => Vars::new(),
        value => serde_json::from_value::<Vars>(value)
            .map_err(|e| ActError::Convert(format!("invalid trigger payload: {e}")))?,
    };
    let started = runtime.start(&workflow, inputs).await;

    // roll the schedule forward even when the start failed, so a failing
    // trigger does not hot-loop on every tick; the error is logged by the
    // caller (at-least-once delivery)
    let mut event = event.clone();
    event.last_run = crate::utils::time::time_millis();
    event.next_run = match event.schedule.as_deref() {
        Some(schedule) => crate::scheduler::core::plan::cron::Cron::next_fire_millis(schedule),
        None => 0,
    };
    runtime.store().events().update(&event).await?;
    started.map(|_| ())
}

impl TimerGuider {
    /// One schedule-trigger pass: fires every due `schedule` trigger row. The
    /// due query is the pass's one store round trip and the only failure that
    /// counts against its health: a trigger whose own fire failed is that
    /// row's problem, and it repeats only when the row itself is unusable
    /// (its model is gone, its payload does not parse) — one such row must
    /// not throttle the schedules that are fine. What the health is about is
    /// reaching the store, and that is what this query decides.
    pub(crate) async fn trigger_tick(&self) {
        // A degraded loop sits this tick out: the store has failed every
        // recent attempt, so another query now would fail too (and log
        // again) — the guard decides when the next attempt is worth making.
        if !self.timers.trigger().attempt() {
            return;
        }
        let store = self.runtime.store();
        let now = crate::utils::time::time_millis();
        let due = match store
            .events()
            .query(
                &crate::query::Query::new().limit(1000).filter(
                    crate::query::Filter::and()
                        .expr(crate::query::Expr::eq("kind", "schedule"))
                        .expr(crate::query::Expr::le("next_run", now)),
                ),
            )
            .await
        {
            Ok(rows) => {
                self.timers.trigger().recovered();
                rows.rows
            }
            Err(err) => {
                error!(error = %err, "schedule query failed");
                self.timers.trigger().failed(&err);
                return;
            }
        };
        for event in due {
            if let Err(err) = fire_schedule(&self.runtime, &event).await {
                error!(event = %event.id, error = %err, "schedule trigger failed");
            }
        }
    }
}
