//! A10: the control-plane loops are independent.
//!
//! These cells move one loop's interval while holding the others still and
//! read the host's own pass counters. The property is scheduling, not business
//! state: a long recovery interval must not gate the ready/timer pass or the
//! outbox retry pass, and a short ready interval must not multiply recovery
//! scans.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::utils::test::{USES_IRQ, USES_MSG};
use crate::{Step, Vars, Workflow};

use super::*;

const PID: &str = "p-a10-split";

/// A one-step workflow whose step waits on the irq act and declares one
/// timeout branch gated on a 100 ms cost window. The branch can only fire
/// from a ready/timer pass: the timer's first round is idle.
fn timed_workflow() -> Workflow {
    let branch = Step::new()
        .with_id("t1")
        .with_if("$cost() >= 100")
        .with_uses(USES_MSG, Vars::new().with("key", "msg1"));
    let mut step = Step::new()
        .with_id("step1")
        .with_uses(USES_IRQ, Vars::new().with("key", "act1"));
    step.timeouts.push(branch);
    let mut workflow = Workflow::new().with_id("wf-a10-split");
    workflow.steps = vec![step];
    workflow
}

fn schedule(
    ready: Duration,
    recovery: Duration,
    outbox: Duration,
    projection: Duration,
) -> KernelSchedule {
    KernelSchedule {
        ready,
        recovery,
        outbox,
        projection,
        ready_budget: PAGE_SIZE,
        recovery_budget: PAGE_SIZE,
        outbox_budget: PAGE_SIZE,
        projection_budget: 32,
    }
}

/// Whether the timeout branch `t1` has a committed generation.
async fn branch_fired(host: &KernelHost, pid: &str) -> bool {
    host.snapshot(pid)
        .await
        .unwrap()
        .is_some_and(|snapshot| snapshot.tasks.keys().any(|id| id.node.as_str() == "t1"))
}

/// The ready/timer loop is the timer's liveness source: with recovery parked
/// at an hour, a due `DispatchTimeout` still fires on the ready cadence.
#[tokio::test(flavor = "multi_thread")]
async fn a_parked_recovery_interval_does_not_delay_a_timer() {
    let (host, wake_rx) = KernelHost::in_memory();
    let shutdown = CancellationToken::new();
    let driver = tokio::spawn(host.clone().serve(
        wake_rx,
        shutdown.clone(),
        schedule(
            Duration::from_millis(25),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        ),
    ));

    let spec = ProcessSpec::from_workflow(PID, &timed_workflow(), &|_| None).unwrap();
    let pid = host.start_process(&spec).await.unwrap();

    let mut fired = false;
    for _ in 0..200 {
        if branch_fired(&host, &pid).await {
            fired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let ready_passes = host.ready_metrics().passes;
    let recovery = host.recovery_metrics();

    shutdown.cancel();
    driver.await.unwrap();

    assert!(
        fired,
        "the timer must fire from the ready pass while recovery is parked"
    );
    assert!(
        ready_passes > 1,
        "the ready/timer loop must have run: {ready_passes} passes"
    );
    assert!(
        recovery.passes <= 1,
        "the parked recovery interval must not be re-scanned: {recovery:?}"
    );
    assert_eq!(
        recovery.scanned, 0,
        "a timer is ready work, never exceptional recovery work: {recovery:?}"
    );
}

/// The runtime's config wiring honors the split: an explicitly configured
/// recovery interval stays parked while the ready pass still fires the timer.
#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_wires_the_configured_loops_independently() {
    let mut config = crate::Config::default();
    config.data.tick_interval_secs = Some(1);
    config.data.ready_interval_secs = Some(1);
    config.data.recovery_interval_secs = Some(3600);
    config.data.projection_interval_secs = Some(3600);
    let engine = crate::Engine::builder()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    let runtime = engine.runtime();
    let pid = runtime.start(&timed_workflow(), Vars::new()).await.unwrap();

    let mut fired = false;
    for _ in 0..500 {
        if branch_fired(runtime.kernel().as_ref(), &pid).await {
            fired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let recovery_passes = runtime.recovery_metrics().passes;
    engine.close().await;

    assert!(fired, "the timer must fire from the configured ready loop");
    assert!(
        recovery_passes <= 1,
        "the parked recovery loop must not follow the ready cadence: {recovery_passes}"
    );
}

/// A short ready interval drives many ready passes without touching the
/// recovery scanner; the outbox retry pass keeps its own cadence too.
#[tokio::test(flavor = "multi_thread")]
async fn a_short_timer_interval_does_not_multiply_recovery_scans() {
    let (host, wake_rx) = KernelHost::in_memory();
    let shutdown = CancellationToken::new();
    let driver = tokio::spawn(host.clone().serve(
        wake_rx,
        shutdown.clone(),
        schedule(
            Duration::from_millis(10),
            Duration::from_secs(3600),
            Duration::from_millis(20),
            Duration::from_secs(3600),
        ),
    ));

    tokio::time::sleep(Duration::from_millis(250)).await;
    let ready = host.ready_metrics();
    let recovery = host.recovery_metrics();
    let outbox = host.outbox_metrics();

    shutdown.cancel();
    driver.await.unwrap();

    assert!(
        ready.passes >= 5,
        "the timer loop ran on its own interval: {ready:?}"
    );
    assert!(
        recovery.passes <= 2,
        "ready passes must not trigger recovery scans: {recovery:?}"
    );
    assert!(
        outbox.passes >= 3,
        "the outbox retry loop ran on its own interval: {outbox:?}"
    );
}
