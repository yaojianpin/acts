//! The store-facing timers' behavioral tests: schedule triggers, the
//! message-retry timer and the snapshot TTL sweep, each pass driven the same
//! way the kernel host's serve loop drives it — one tick, one pass.

use std::sync::Arc;

use crate::scheduler::Runtime;
use crate::scheduler::shell::timers::{TimerGuider, fire_schedule};
use crate::{Config, Vars, Workflow};

fn runtime() -> Arc<Runtime> {
    Runtime::new(&Config::default(), None).unwrap()
}

fn guider(runtime: &Arc<Runtime>) -> Arc<TimerGuider> {
    Arc::new(TimerGuider::new(runtime))
}

/// A store that fails exactly the next canonical-message point read. The
/// delivery scan still works, so the retry pass reaches the per-row message
/// lookup and must distinguish "the message is gone" from "the read failed".
struct MessageReadFaultKv {
    inner: crate::store::MemoryStore,
    armed: std::sync::atomic::AtomicBool,
}

impl MessageReadFaultKv {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: crate::store::MemoryStore::new(),
            armed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl crate::store::KvStore for MessageReadFaultKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        if key.starts_with("messages-")
            && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(crate::ActError::Store(
                "injected message read failure".into(),
            ));
        }
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> crate::Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(
        &self,
        ops: &[crate::store::StoreBatchOp],
        guards: &[crate::store::StoreGuard],
    ) -> crate::Result<bool> {
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: crate::store::ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
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
    fire_schedule(&runtime, &event).await.unwrap();
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
    fire_schedule(&runtime, &event).await.unwrap();
    let stored = runtime.store().events().find(&event.id).await.unwrap();
    assert!(stored.next_run > stored.last_run);
    runtime.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_trigger_pass_survives_a_failing_fire() {
    let runtime = runtime();
    // A due schedule row whose model does not exist: `fire_schedule` fails
    // and the pass logs it instead of failing the loop's health.
    let event = event("missing-model", Some("* * * * * *"));
    runtime.store().events().create(&event).await.unwrap();
    guider(&runtime).trigger_tick().await;
    // The failing row is still due (the start never rolled it), so it is
    // retried on the next tick; the pass survived and health saw the store
    // succeed.
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
        ..Default::default()
    };
    runtime
        .store()
        .deliveries()
        .create(&delivery)
        .await
        .unwrap();
    guider(&runtime).retry_tick().await;
    assert!(
        runtime
            .store()
            .deliveries()
            .find_opt(&delivery.id)
            .await
            .unwrap()
            .is_none(),
        "the orphan delivery is dropped in the same pass that notices it"
    );
    runtime.close().await;
}

/// A transient failure to read the canonical message is not proof of an
/// orphan. The retry pass must keep the delivery row (the responsibility is
/// still open) and report the pass degraded; deleting here would erase a real
/// client's unacked delivery on one I/O error.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sch_timers_retry_keeps_a_delivery_when_the_message_read_fails() {
    use crate::store::data::{Delivery, DeliveryStatus, Message};
    use std::sync::Arc as StdArc;

    let kv = MessageReadFaultKv::new();
    let kv_handle: StdArc<dyn crate::store::KvStore> = kv.clone();
    let runtime = Runtime::new(&Config::default(), Some(kv_handle)).unwrap();
    let now = crate::utils::time::time_millis();
    runtime
        .store()
        .messages()
        .create(&Message {
            id: "m1".into(),
            pid: "p".into(),
            tid: "t".into(),
            ..Message::default()
        })
        .await
        .unwrap();
    let delivery = Delivery {
        id: "d1".into(),
        msg_id: "m1".into(),
        pid: "p".into(),
        tid: "t".into(),
        chan_id: "chan".into(),
        chan_pattern: "*".into(),
        status: DeliveryStatus::Created,
        retry_times: 0,
        create_time: now,
        update_time: now - 60_000,
        timestamp: now,
        ..Delivery::default()
    };
    runtime
        .store()
        .deliveries()
        .create(&delivery)
        .await
        .unwrap();

    kv.arm();
    guider(&runtime).retry_tick().await;
    assert!(
        runtime
            .store()
            .deliveries()
            .find_opt(&delivery.id)
            .await
            .unwrap()
            .is_some(),
        "a message read error must not delete the delivery"
    );
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
    guider(&runtime).purge_tick().await;
    assert!(
        store.list().is_empty(),
        "the expired snapshot entry is purged in the same pass"
    );
    runtime.close().await;
}
