//! SUBFLOW scheduler cells: the parent act parks, the child run is its own
//! aggregate, and only the child's return lets the parent's next step run.
//!
//! The cells here drive the flow through the same `FlowFixture` as the other
//! functional scheduler tests; the failure/restart/duplicate matrix lives under
//! `scheduler::tests::reliability::subflow`.

use serial_test::serial;

use super::*;
use crate::{Step, utils::test::USES_SET};

/// The parent waits for the child before its next step, folds the child's
/// outputs into the parent scope, and announces the steps in that order.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_parks_until_the_child_returns_and_then_flows() {
    let mut parent = Workflow::new().with_id("flow-subflow");
    parent.steps = vec![
        Step::new().with_id("sub").with_uses(
            "acts.core.subflow",
            Vars::new().with("to", "flow-subflow-child"),
        ),
        Step::new()
            .with_id("after")
            .with_uses(USES_SET, Vars::new().with("seen", "${{ v }}")),
    ];
    let mut fixture = FlowFixture::deploy(&parent).await;

    let child = Workflow::new()
        .with_id("flow-subflow-child")
        .with_step(|step| {
            step.with_id("child")
                .with_uses(crate::utils::test::USES_IRQ, Vars::new().with("key", "c1"))
        });
    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .model()
        .deploy(&child, None)
        .await
        .unwrap();

    let pid = fixture.start(Vars::new()).await;
    fixture.wait_state("sub", TaskState::Waiting).await;

    // The child is a run of its own, waiting on its Irq act; the parent's next
    // step must not have started.
    let created = fixture.wait_created("child").await;
    assert_ne!(created.pid, pid, "the child is a separate run");
    assert_eq!(
        fixture.state("after").await,
        TaskState::Created,
        "the parent's next step must not start before the child returns"
    );
    assert!(
        !fixture
            .snapshot()
            .await
            .tasks
            .values()
            .all(|task| task.state.is_terminal()),
        "the parent is still in flight while the child waits"
    );

    // Complete the child act; its return completes the parent's subflow act,
    // which is what admits `after`.
    fixture
        .act(
            &created.pid,
            &created.tid,
            EventAction::Next,
            Vars::new().with("v", 100),
        )
        .await
        .unwrap();

    fixture.wait_state("after", TaskState::Completed).await;
    fixture.wait_state("sub", TaskState::Completed).await;
    fixture.wait_settled().await;

    assert_eq!(
        fixture.outputs().get_value("seen"),
        Some(&serde_json::json!(100)),
        "the child's v rides the return into the parent's next step"
    );

    // The channel order the scheduler produced: the child's act completes
    // before the parent's next step is announced.
    fixture.wait_message("child", MessageState::Completed).await;
    fixture.wait_message("after", MessageState::Created).await;
    let child_completed = fixture
        .messages()
        .iter()
        .position(|message| {
            message.pid == created.pid
                && message.nid == "child"
                && message.state == MessageState::Completed
        })
        .expect("the child act completion was announced");
    let after_created = fixture
        .messages()
        .iter()
        .position(|message| message.nid == "after" && message.state == MessageState::Created)
        .expect("the parent's next act was announced");
    assert!(
        child_completed < after_created,
        "the return must precede the parent's next act: {:?}",
        fixture.messages()
    );

    fixture.close().await;
}

/// A child that errors returns `Fail`: the parent's subflow act is failed and
/// its next step never runs.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_subflow_failure_never_runs_the_next_step() {
    let mut parent = Workflow::new().with_id("flow-subflow-error");
    parent.steps = vec![
        Step::new().with_id("sub").with_uses(
            "acts.core.subflow",
            Vars::new().with("to", "flow-subflow-error-child"),
        ),
        Step::new()
            .with_id("after")
            .with_uses(USES_SET, Vars::new().with("seen", 1)),
    ];
    let mut fixture = FlowFixture::deploy(&parent).await;

    let child = Workflow::new()
        .with_id("flow-subflow-error-child")
        .with_step(|step| {
            step.with_id("child")
                .with_uses(crate::utils::test::USES_IRQ, Vars::new().with("key", "c1"))
        });
    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .model()
        .deploy(&child, None)
        .await
        .unwrap();

    fixture.start(Vars::new()).await;
    fixture.wait_state("sub", TaskState::Waiting).await;
    let created = fixture.wait_created("child").await;

    fixture
        .fail(&created.pid, &created.tid, "E1", "boom")
        .await
        .unwrap();

    fixture.wait_state("sub", TaskState::Failed).await;
    poll_until("the parent root to fail", || async {
        fixture
            .snapshot()
            .await
            .tasks
            .iter()
            .find(|(id, _)| id.node.as_str() == "$")
            .map(|(_, task)| task.state)
            == Some(TaskState::Failed)
    })
    .await;

    assert_ne!(
        fixture.state("after").await,
        TaskState::Completed,
        "the parent's next step must never run"
    );
    assert!(
        fixture
            .messages_of("after")
            .iter()
            .all(|message| message.state != MessageState::Created),
        "a failed return never admits the next step"
    );

    fixture.close().await;
}
