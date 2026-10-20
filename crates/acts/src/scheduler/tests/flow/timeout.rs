//! Timeout cells migrated from the pre-kernel scheduler suite (`step/timeout`
//! and the timeout rows of the legacy tick tests): a timed-out step dispatches
//! its branches in declaration order, a branch fires at most once, and the
//! step waits for a fired branch before it completes.

use serial_test::serial;

use crate::{
    MessageState, Vars, Workflow,
    scheduler::{core::model::task_graph::TaskState, runtime::TEST_TICK_MS},
    utils::test::{USES_IRQ, USES_MSG},
};

use super::FlowFixture;

/// The first timeout branch whose window is open fires once; the timer then
/// retires instead of dispatching it again on every tick.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_timeout_a_branch_fires_once_and_the_owner_waits_for_it() {
    let workflow = Workflow::new()
        .with_id("flow-timeout-once")
        .with_step(|step| {
            step.with_id("wait")
                .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
                .with_timeout(|timeout| {
                    timeout
                        .with_id("t1")
                        .with_if("$cost() >= 0")
                        .with_uses(USES_MSG, Vars::new().with("key", "timeout"))
                })
        })
        .with_step(|step| step.with_id("after"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let fired = fixture.wait_message("t1", MessageState::Completed).await;
    assert_eq!(fired.nid, "t1");
    assert_eq!(fixture.state("t1").await, TaskState::Completed);
    assert_eq!(
        fixture.state("wait").await,
        TaskState::Waiting,
        "the act still waits for its client"
    );

    // No second dispatch of the same one-shot branch on later ticks: the
    // branch's msg act is delivered exactly once.
    tokio::time::sleep(std::time::Duration::from_millis(TEST_TICK_MS * 3)).await;
    assert_eq!(
        fixture.announced("t1"),
        vec![MessageState::Completed],
        "a timeout branch fires once: its single delivery is the completion"
    );

    let act = fixture.wait_created("wait").await;
    fixture.next(&act.pid, &act.tid).await.unwrap();
    fixture.wait_completed().await;
    assert_eq!(fixture.state("wait").await, TaskState::Completed);
    assert_eq!(fixture.state("after").await, TaskState::Completed);
    fixture.close().await;
}

/// Only a branch whose window is open fires: an open `$cost() >= 0` branch is
/// claimed and its later closed sibling (`$cost_in('1d')`) never dispatches.
/// The dispatch order is declaration order (the legacy rule), so the open
/// branch is declared first.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_timeout_only_the_open_window_dispatches() {
    let workflow = Workflow::new()
        .with_id("flow-timeout-guards")
        .with_step(|step| {
            step.with_id("wait")
                .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
                .with_timeout(|timeout| {
                    timeout
                        .with_id("t_fire")
                        .with_if("$cost() >= 0")
                        .with_uses(USES_MSG, Vars::new().with("key", "fire"))
                })
                .with_timeout(|timeout| {
                    timeout
                        .with_id("t_never")
                        .with_if("$cost_in('1d')")
                        .with_uses(USES_MSG, Vars::new().with("key", "never"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture
        .wait_message("t_fire", MessageState::Completed)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(TEST_TICK_MS * 2)).await;
    assert!(
        fixture.messages_of("t_never").is_empty(),
        "a closed window is not dispatched: {:?}",
        fixture.messages_of("t_never")
    );

    let act = fixture.wait_created("wait").await;
    fixture.next(&act.pid, &act.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A fired branch that is still waiting holds the owner open: the step's act
/// completed, but the step is parked on its timeout child until that child
/// settles (the legacy `auto_complete` counts every child).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_timeout_the_owner_completes_only_after_the_fired_branch_settles() {
    let workflow = Workflow::new()
        .with_id("flow-timeout-owner")
        .with_step(|step| {
            step.with_id("wait")
                .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
                .with_timeout(|timeout| {
                    timeout
                        .with_id("t_hold")
                        .with_if("$cost() >= 0")
                        .with_uses(USES_IRQ, Vars::new().with("key", "timeout"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let branch = fixture.wait_created("t_hold").await;
    assert_eq!(fixture.state("t_hold").await, TaskState::Waiting);

    let act = fixture.wait_created("wait").await;
    fixture.next(&act.pid, &act.tid).await.unwrap();
    fixture.wait_message("wait", MessageState::Completed).await;
    assert_eq!(
        fixture.state("wait").await,
        TaskState::Running,
        "the step is parked on its timeout child, not completed"
    );

    fixture.next(&branch.pid, &branch.tid).await.unwrap();
    fixture.wait_completed().await;
    assert_eq!(fixture.state("wait").await, TaskState::Completed);
    assert_eq!(fixture.state("t_hold").await, TaskState::Completed);
    fixture.close().await;
}
