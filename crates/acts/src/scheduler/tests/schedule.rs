//! `schedule` triggers: the runtime's cron timer, end to end.
//!
//! These are the pre-kernel schedule cells, migrated to the kernel engine:
//! deployment arms a trigger row to its next cron fire, the trigger timer fires
//! every due row and rolls it forward, a restart resumes from the same durable
//! row, and the loop degrades (and recovers) with the store it reads. The cells
//! keep the legacy goals — one due moment starts exactly one process, a restart
//! neither loses nor duplicates the row, and a failing store is reported rather
//! than swallowed — and observe the current implementation's own surfaces: the
//! `events` row, the projected `procs` rows and the engine's lifecycle events.
//!
//! [`ScheduleFixture`] is the shared harness: it deploys a model with its
//! schedule trigger, waits for arming / firing / processes / completed runs,
//! can make a row due early, redeploy a changed model, and restart the engine
//! over the same store. A cell therefore states only what it observes, never
//! how to poll for it.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serial_test::serial;

use crate::{
    Engine, EventInfo, MemoryStore, Message, Principal, Trigger, Vars, Workflow,
    query::{Expr, Filter, Query},
    scheduler::runtime::TEST_TICK_MS,
    scheduler::shell::health::{DEGRADE_AFTER, HealthState},
    utils,
    utils::test::USES_IRQ,
};

use super::reliability::{OutageKv, bounded, engine_on, poll_until, poll_until_dumped};

/// The shared harness for one deployed model with a `schedule` trigger.
///
/// Preparation is one call (`deploy`), the model's trigger id is derived from
/// the declaration, and every observation a cell needs is a method: a fire
/// (`wait_fire`), live processes (`wait_live_processes`), completed runs
/// (`wait_completed`) and the trigger row itself (`row`). `restart` rebuilds
/// the engine over the same store, and `redeploy` re-runs the trigger
/// reconciliation, so a cell never re-implements engine setup or polling.
pub(crate) struct ScheduleFixture {
    kv: Arc<dyn crate::KvStore>,
    engine: Engine,
    model_id: String,
    trigger_id: String,
    /// Completion events of this engine, in order (recorded from the moment
    /// the engine exists, so the first scheduled run cannot race the sink).
    completed: Arc<Mutex<Vec<Message>>>,
}

impl ScheduleFixture {
    /// Deploy `workflow` (which must declare one `schedule` trigger) on a fresh
    /// in-memory store.
    pub(crate) async fn deploy(workflow: &Workflow) -> Self {
        Self::deploy_on(Arc::new(MemoryStore::new()), workflow).await
    }

    /// Deploy `workflow` on a caller-owned store: the cell can then arm faults
    /// or restart the engine over the same durable rows.
    pub(crate) async fn deploy_on(kv: Arc<dyn crate::KvStore>, workflow: &Workflow) -> Self {
        let engine = engine_on(kv.clone()).await;
        let completed = install_completion_sink(&engine);
        let fixture = Self {
            kv,
            engine,
            model_id: workflow.id.clone(),
            trigger_id: schedule_trigger_id(workflow),
            completed,
        };
        fixture.deploy_workflow(workflow).await;
        fixture
    }

    /// Replace the engine with a fresh one over the same store, exactly like a
    /// process restart: the durable `events` row and the aggregates are the
    /// only state that survives.
    pub(crate) async fn restart(&mut self) {
        self.engine.close().await;
        self.engine = engine_on(self.kv.clone()).await;
        install_completion_sink_into(&self.engine, self.completed.clone());
    }

    /// Reconcile the deployed model with `workflow` (the deploy path updates the
    /// row in place and re-arms a changed cron).
    pub(crate) async fn redeploy(&self, workflow: &Workflow) {
        self.manager().model().deploy(workflow, None).await.unwrap();
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn store(&self) -> Arc<crate::Store> {
        self.engine.runtime().store()
    }

    pub(crate) fn trigger_id(&self) -> &str {
        &self.trigger_id
    }

    pub(crate) fn manager(&self) -> Arc<crate::Executor> {
        self.engine.executor(&Principal::unrestricted())
    }

    /// The trigger's projected row (`evt:get`), the timer's durable state.
    pub(crate) async fn row(&self) -> EventInfo {
        self.manager().evt().get(&self.trigger_id).await.unwrap()
    }

    /// Wait until deployment armed the row (the next cron fire is in the
    /// future) and answer the armed row.
    pub(crate) async fn wait_armed(&self) -> EventInfo {
        let what = format!("schedule {} to arm on deploy", self.trigger_id);
        poll_until(&what, || async { self.row().await.next_run > 0 }).await;
        self.row().await
    }

    /// Wait until the timer records a fire after `after` (`last_run`) and rolls
    /// `next_run` past it, then answer the row.
    pub(crate) async fn wait_fire(&self, after: i64) -> EventInfo {
        let what = format!(
            "schedule {} to fire (last_run > {after}) and roll forward",
            self.trigger_id
        );
        poll_until_dumped(
            &what,
            || async {
                let row = self.row().await;
                row.last_run > after && row.next_run > row.last_run
            },
            || async { format!("trigger row: {:?}", self.row().await) },
        )
        .await;
        self.row().await
    }

    /// The live processes the schedule started (the projected `procs` rows).
    pub(crate) async fn live_processes(&self) -> usize {
        self.store()
            .procs()
            .query(
                &Query::new()
                    .limit(1000)
                    .filter(Filter::and().expr(Expr::eq("mid", self.model_id.clone()))),
            )
            .await
            .unwrap()
            .count
    }

    /// Wait until `n` live processes of this model exist.
    pub(crate) async fn wait_live_processes(&self, n: usize) {
        let what = format!("{n} live process(es) of {}", self.model_id);
        poll_until(&what, || async { self.live_processes().await >= n }).await;
    }

    /// Wait until `n` runs of this model reported completion and answer their
    /// lifecycle messages.
    pub(crate) async fn wait_completed(&self, n: usize) -> Vec<Message> {
        let what = format!("{n} completed run(s) of {}", self.model_id);
        poll_until(&what, || async { self.completed.lock().len() >= n }).await;
        self.completed.lock().clone()
    }

    /// Make the row due in the past without recording a fire — the crash
    /// window between starting a process and rolling the row.
    pub(crate) async fn make_due(&self) {
        let mut evt = self.store().events().find(&self.trigger_id).await.unwrap();
        evt.next_run = utils::time::time_millis() - 1000;
        self.store().events().update(&evt).await.unwrap();
    }

    /// How many `schedule` rows this model has (a restart must not duplicate
    /// one, or every tick would fire twice).
    pub(crate) async fn schedule_rows(&self) -> usize {
        self.store()
            .events()
            .query(
                &Query::new().limit(1000).filter(
                    Filter::and()
                        .expr(Expr::eq("kind", "schedule"))
                        .expr(Expr::eq("mid", self.model_id.clone())),
                ),
            )
            .await
            .unwrap()
            .count
    }

    pub(crate) async fn close(self) {
        self.engine.close().await;
    }

    async fn deploy_workflow(&self, workflow: &Workflow) {
        self.manager().model().deploy(workflow, None).await.unwrap();
    }
}

/// Record every completion of `engine` into a shared list and return it.
fn install_completion_sink(engine: &Engine) -> Arc<Mutex<Vec<Message>>> {
    let completed = Arc::new(Mutex::new(Vec::new()));
    install_completion_sink_into(engine, completed.clone());
    completed
}

fn install_completion_sink_into(engine: &Engine, completed: Arc<Mutex<Vec<Message>>>) {
    engine.channel().on_complete(move |e| {
        let completed = completed.clone();
        async move {
            completed.lock().push(e.inner().clone());
        }
    });
}

/// The event id of the model's first `schedule` trigger (`<mid>:<trigger>`).
fn schedule_trigger_id(workflow: &Workflow) -> String {
    let trigger = workflow
        .on
        .iter()
        .find(|trigger| trigger.kind == crate::TRIGGER_SCHEDULE)
        .unwrap_or_else(|| panic!("model {} declares no schedule trigger", workflow.id));
    format!("{}:{}", workflow.id, trigger.id)
}

/// A one-step model whose step waits on an irq act nobody answers: every fire
/// leaves exactly one more live process, so process counts are the fire count.
pub(crate) fn scheduled_irq_model(model_id: &str, cron: &str) -> Workflow {
    let mut model = Workflow::new().with_id(model_id).with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    model.on = vec![schedule_trigger(cron, serde_json::Value::Null)];
    model
}

/// A one-step model that completes on its own; `params` become the scheduled
/// run's start inputs.
pub(crate) fn scheduled_model(model_id: &str, cron: &str, params: serde_json::Value) -> Workflow {
    let mut model = Workflow::new()
        .with_id(model_id)
        .with_step(|step| step.with_id("s1"));
    model.on = vec![schedule_trigger(cron, params)];
    model
}

fn schedule_trigger(cron: &str, params: serde_json::Value) -> Trigger {
    Trigger {
        id: "tick".to_string(),
        name: "tick".to_string(),
        kind: crate::TRIGGER_SCHEDULE.to_string(),
        params,
        schedule: Some(cron.to_string()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Deploy, arm, fire and completion
// ---------------------------------------------------------------------------

/// Deployment arms the row to its next cron fire; the timer then fires it with
/// the trigger's default params, rolls the row forward, and the started run
/// completes with those params as its inputs. This is the legacy
/// `export_trigger_schedule_auto_fire` goal, observed end to end on the kernel.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_trigger_fires_with_its_params_and_the_run_completes() {
    bounded(
        "sch_schedule_trigger_fires_with_its_params_and_the_run_completes",
        a_schedule_trigger_fires_with_its_params_and_the_run_completes_inner(),
    )
    .await;
}

async fn a_schedule_trigger_fires_with_its_params_and_the_run_completes_inner() {
    let model = scheduled_model("sch-fire", "* * * * * *", serde_json::json!({"a": 7}));
    let fixture = ScheduleFixture::deploy(&model).await;

    let armed = fixture.wait_armed().await;
    assert_eq!(armed.kind, "schedule");
    assert!(armed.next_run > 0, "deploy must arm the row");

    let fired = fixture.wait_fire(armed.last_run).await;
    let completed = fixture.wait_completed(1).await;
    let message = completed.first().expect("the scheduled run must complete");
    assert_eq!(message.mid, "sch-fire");
    assert_eq!(
        message.inputs.get::<i32>("a"),
        Some(7),
        "the trigger params are the scheduled run's inputs"
    );
    // The lifecycle completion message carries the run's window, so a
    // consumer's `cost()` is the run's duration.
    assert!(message.start_time > 0);
    assert!(message.end_time >= message.start_time);
    assert!(fired.last_run > armed.last_run);
    assert!(fired.next_run > fired.last_run);

    fixture.close().await;
}

/// A schedule trigger has no caller: firing it through `evt:start` is refused
/// (the legacy `export_trigger_schedule_cannot_start_manually` goal).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_trigger_cannot_be_started_manually() {
    bounded(
        "sch_schedule_trigger_cannot_be_started_manually",
        a_schedule_trigger_cannot_be_started_manually_inner(),
    )
    .await;
}

async fn a_schedule_trigger_cannot_be_started_manually_inner() {
    let fixture = ScheduleFixture::deploy(&scheduled_model(
        "sch-manual",
        "0 0 3 * * *",
        serde_json::json!({}),
    ))
    .await;
    fixture.wait_armed().await;

    assert!(
        fixture
            .manager()
            .evt()
            .start(fixture.trigger_id(), &Vars::new().into())
            .await
            .is_err(),
        "a schedule trigger may only be fired by the timer"
    );

    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Redeploy: row reconciliation keeps the run state
// ---------------------------------------------------------------------------

/// A redeploy keeps the schedule's run state unless the cron itself changed:
/// params-only changes update the row in place, and a changed cron re-arms
/// `next_run` while keeping `last_run` (the legacy
/// `export_trigger_redeploy_keeps_schedule_state` goal).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_redeploy_preserves_schedule_state_and_rearms_a_changed_cron() {
    bounded(
        "sch_schedule_redeploy_preserves_schedule_state_and_rearms_a_changed_cron",
        a_redeploy_preserves_schedule_state_and_rearms_a_changed_cron_inner(),
    )
    .await;
}

async fn a_redeploy_preserves_schedule_state_and_rearms_a_changed_cron_inner() {
    let fixture = ScheduleFixture::deploy(&scheduled_model(
        "sch-redeploy",
        "0 0 12 * * *",
        serde_json::json!({"a": 1}),
    ))
    .await;
    let first = fixture.wait_armed().await;

    // params-only change: the row updates, the run state is preserved
    let before = fixture.row().await;
    fixture
        .redeploy(&scheduled_model(
            "sch-redeploy",
            "0 0 12 * * *",
            serde_json::json!({"a": 2}),
        ))
        .await;
    let after = fixture.row().await;
    assert_eq!(after.params, "{\"a\":2}");
    assert_eq!(after.next_run, before.next_run, "schedule state preserved");
    assert_eq!(after.last_run, before.last_run);
    assert_eq!(after.next_run, first.next_run);

    // cron change: `last_run` is kept, `next_run` re-arms to the new cron
    let armed_at = utils::time::time_millis();
    fixture
        .redeploy(&scheduled_model(
            "sch-redeploy",
            "0 0 6 * * *",
            serde_json::json!({"a": 2}),
        ))
        .await;
    let rearmed = fixture.row().await;
    assert_eq!(rearmed.last_run, after.last_run, "last_run kept");
    assert!(
        rearmed.next_run >= armed_at,
        "a changed cron must re-arm next_run"
    );
    assert_ne!(rearmed.next_run, after.next_run);

    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Restart
// ---------------------------------------------------------------------------

/// A deployed schedule trigger keeps firing after the engine is replaced over
/// the same store: the single row advances and new processes appear, and the
/// restart does not duplicate the row (a second row would double-fire).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_trigger_fires_after_an_engine_restart() {
    bounded(
        "sch_schedule_trigger_fires_after_an_engine_restart",
        a_schedule_trigger_fires_after_an_engine_restart_inner(),
    )
    .await;
}

async fn a_schedule_trigger_fires_after_an_engine_restart_inner() {
    let mut fixture =
        ScheduleFixture::deploy(&scheduled_irq_model("sch-restart", "* * * * * *")).await;
    let trigger_id = fixture.trigger_id().to_string();

    let first_fire = fixture.wait_fire(0).await;
    let last_run_first = first_fire.last_run;
    fixture.wait_live_processes(1).await;
    let n1 = fixture.live_processes().await;

    fixture.restart().await;

    let resumed = fixture.wait_fire(last_run_first).await;
    assert!(
        resumed.next_run > resumed.last_run,
        "the restarted timer must roll the schedule forward"
    );
    fixture.wait_live_processes(n1 + 1).await;

    assert_eq!(
        fixture.schedule_rows().await,
        1,
        "the restart must not duplicate the schedule row"
    );
    assert_eq!(fixture.row().await.id, trigger_id);

    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Duplicate due moment
// ---------------------------------------------------------------------------

/// One due moment starts exactly one process: a row that surfaces due again
/// (the crash window between starting the process and rolling the row) is
/// rolled forward inside the fire, so ticks do not turn it into a burst.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_duplicate_due_schedule_fire_starts_exactly_one_process() {
    bounded(
        "sch_schedule_duplicate_due_schedule_fire_starts_exactly_one_process",
        a_duplicate_due_schedule_fire_starts_exactly_one_process_inner(),
    )
    .await;
}

async fn a_duplicate_due_schedule_fire_starts_exactly_one_process_inner() {
    let fixture =
        ScheduleFixture::deploy(&scheduled_irq_model("sch-duplicate", "0 0 3 * * *")).await;
    let armed = fixture.wait_armed().await;
    assert!(
        armed.next_run > utils::time::time_millis(),
        "deploy must arm the schedule in the future"
    );

    // a far-future schedule: ticks alone start nothing
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(fixture.live_processes().await, 0);

    // the same due moment surfaces (crash between start and row roll)
    fixture.make_due().await;
    fixture.wait_live_processes(1).await;
    let fired = fixture.row().await;
    assert!(fired.last_run > 0, "the fire must be recorded");
    assert!(
        fired.next_run > utils::time::time_millis(),
        "the fire must roll the row to the next cron time"
    );

    // more ticks: the consumed moment does not repeat itself
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        fixture.live_processes().await,
        1,
        "one due moment must start exactly one process"
    );
    let settled = fixture.row().await;
    assert_eq!(
        settled.last_run, fired.last_run,
        "no fire may happen while the row is not due"
    );

    // a second due moment: again exactly one process (1 due : 1 process)
    fixture.make_due().await;
    fixture.wait_live_processes(2).await;
    tokio::time::sleep(Duration::from_millis(TEST_TICK_MS * 2)).await;
    assert_eq!(fixture.live_processes().await, 2);
    let settled = fixture.row().await;
    assert!(settled.last_run > fired.last_run);
    assert!(settled.next_run > utils::time::time_millis());

    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Store outage
// ---------------------------------------------------------------------------

/// A store that keeps failing the trigger timer's due query must leave the
/// loop reporting itself `Degraded` (from `DEGRADE_AFTER` consecutive failed
/// ticks it backs off instead of hammering the store), and the first clean
/// tick after the store answers must bring it back to `Healthy` — after which
/// it fires a due row again.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_schedule_trigger_loop_degrades_under_a_down_store_and_recovers() {
    bounded(
        "sch_schedule_trigger_loop_degrades_under_a_down_store_and_recovers",
        the_trigger_loop_degrades_under_a_down_store_and_recovers_inner(),
    )
    .await;
}

async fn the_trigger_loop_degrades_under_a_down_store_and_recovers_inner() {
    let kv = OutageKv::new();
    let fixture = ScheduleFixture::deploy_on(
        kv.store(),
        &scheduled_irq_model("sch-outage", "0 0 3 * * *"),
    )
    .await;
    fixture.wait_armed().await;
    assert_eq!(fixture.live_processes().await, 0);

    kv.down();
    let reads_before = kv.reads();
    let what = "the trigger loop to report degraded while its store reads fail";
    poll_until(what, || async {
        let health = fixture.engine().runtime().scheduler_health().trigger;
        health.state == HealthState::Degraded && health.backoff_ticks > 0
    })
    .await;
    let health = fixture.engine().runtime().scheduler_health().trigger;
    assert!(
        health.consecutive_failures >= DEGRADE_AFTER,
        "a degrading loop must have failed at least {DEGRADE_AFTER} ticks in a row: {health:?}"
    );
    assert!(kv.reads() > reads_before);

    kv.heal();
    let what = "the trigger loop to report Healthy again once the store answers";
    poll_until(what, || async {
        let health = fixture.engine().runtime().scheduler_health().trigger;
        health.state == HealthState::Healthy
            && health.consecutive_failures == 0
            && health.backoff_ticks == 0
    })
    .await;

    // the recovered loop is live: it fires a row that is now due
    fixture.make_due().await;
    fixture.wait_live_processes(1).await;
    let fired = fixture.row().await;
    assert!(fired.last_run > 0, "the fire must be recorded");

    fixture.close().await;
}
