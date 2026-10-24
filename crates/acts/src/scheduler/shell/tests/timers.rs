//! The store-facing timer loops' behavioral tests: schedule triggers, the
//! message-retry timer and the snapshot TTL sweep, each driven through the
//! runtime the same way the production build drives them.

use std::sync::Arc;
use std::time::Duration;

use crate::scheduler::Runtime;
use crate::{Config, Vars, Workflow};

fn runtime() -> Arc<Runtime> {
    Runtime::new(&Config::default(), None).unwrap()
}

async fn insert_model(runtime: &Runtime, workflow: &Workflow) {
    runtime
        .store()
        .models()
        .create(&crate::store::data::Model {
            id: workflow.id.clone(),
            name: workflow.name.clone(),
            desc: String::new(),
            ver: "0.1.0".into(),
            size: 0,
            create_time: 0,
            update_time: 0,
            data: serde_json::to_string(workflow).unwrap(),
            view: None,
            timestamp: 0,
            v: 0,
        })
        .await
        .unwrap();
}

fn event(mid: &str, schedule: Option<&str>) -> crate::store::data::Event {
    crate::store::data::Event {
        id: format!("event-{}", crate::utils::longid()),
        name: String::new(),
        mid: mid.into(),
        ver: "0.1.0".into(),
        kind: "schedule".into(),
        params: "null".into(),
        schedule: schedule.map(str::to_string),
        last_run: 0,
        next_run: 0,
        create_time: 0,
        timestamp: 0,
        v: 1,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_trigger_without_a_schedule_rolls_forward_to_zero() {
    let runtime = runtime();
    let workflow = Workflow::new()
        .with_id("wf-trigger-none")
        .with_step(|step| step.with_id("s1"));
    insert_model(&runtime, &workflow).await;
    let event = event(&workflow.id, None);
    runtime.store().events().create(&event).await.unwrap();
    crate::scheduler::shell::timers::fire_schedule(&runtime, &event)
        .await
        .unwrap();
    let stored = runtime.store().events().find(&event.id).await.unwrap();
    assert_eq!(stored.next_run, 0, "no cron expression means not armed");
    assert!(stored.last_run > 0);
    runtime.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_trigger_with_a_schedule_arms_its_next_fire() {
    let runtime = runtime();
    let workflow = Workflow::new()
        .with_id("wf-trigger-cron")
        .with_step(|step| step.with_id("s1"));
    insert_model(&runtime, &workflow).await;
    let event = event(&workflow.id, Some("* * * * * *"));
    runtime.store().events().create(&event).await.unwrap();
    crate::scheduler::shell::timers::fire_schedule(&runtime, &event)
        .await
        .unwrap();
    let stored = runtime.store().events().find(&event.id).await.unwrap();
    assert!(stored.next_run > stored.last_run);
    runtime.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_trigger_logs_a_failing_fire_and_keeps_ticking() {
    let runtime = runtime();
    // A due schedule row whose model does not exist: `fire_schedule` fails
    // and the timer loop logs it instead of dying.
    let event = event("missing-model", Some("* * * * * *"));
    runtime.store().events().create(&event).await.unwrap();
    runtime.init_trigger_timer();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The failing row is still due (the start never rolled it), so it is
    // retried; the loop survived and health saw the store succeed.
    assert!(runtime.scheduler_health().trigger.consecutive_failures == 0);
    runtime.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_retry_drops_a_delivery_without_a_canonical_message() {
    use crate::store::data::{Delivery, DeliveryStatus};

    let runtime = runtime();
    let now = crate::utils::time::time_millis();
    let delivery = Delivery {
        id: format!("orphan-{}", crate::utils::longid()),
        msg_id: format!("missing-{}", crate::utils::longid()),
        pid: "p".into(),
        tid: "t".into(),
        chan_id: "chan".into(),
        chan_pattern: "*".into(),
        status: DeliveryStatus::Created,
        retry_times: 0,
        create_time: 0,
        update_time: 0,
        timestamp: now,
        v: 1,
    };
    runtime
        .store()
        .deliveries()
        .create(&delivery)
        .await
        .unwrap();
    runtime.init_retry_timer().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if runtime
            .store()
            .deliveries()
            .find_opt(&delivery.id)
            .await
            .unwrap()
            .is_none()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the orphan delivery was never dropped"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    runtime.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_snapshot_purges_expired_entries() {
    let runtime = runtime();
    let registry = runtime.snapshot_registry();
    let store = registry.register(
        "ttl",
        crate::snapshot::SnapshotOptions::default().with_ttl(0),
    );
    store.upsert("scope", 1, Vars::new().with("a", 1));
    runtime.init_snapshot_timer();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if store.list().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the expired snapshot entry was never purged"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    runtime.close().await;
}
