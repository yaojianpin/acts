//! `acts.core.irq`: the request act waits for the client and carries its
//! params; every client decision settles it (`pack_irq_*`).

use serial_test::serial;

use crate::{MessageState, Vars, event::EventAction, scheduler::TaskState};

use super::{FlowFixture, USES_IRQ, one_step};

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_irq_waits_for_the_client_and_carries_its_params() {
    let workflow = one_step(
        "pack-irq-wait",
        USES_IRQ,
        Vars::new().with("key", "act1").with("a", 1),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    assert!(created.is_irq());
    assert_eq!(created.params().unwrap().get::<i32>("a"), Some(1));
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.wait_completed().await;
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    fixture.close().await;
}

/// The irq act's state follows the client's decision, not a timer: submit,
/// skip, abort and error each settle it (the legacy `pack_irq_*_action`
/// cases).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_irq_client_decisions_settle_the_act() {
    for (id, action, state, options, completed) in [
        (
            "submit",
            EventAction::Submit,
            TaskState::Submitted,
            Vars::new(),
            true,
        ),
        (
            "skip",
            EventAction::Skip,
            TaskState::Skipped,
            Vars::new(),
            true,
        ),
        (
            "abort",
            EventAction::Abort,
            TaskState::Aborted,
            Vars::new(),
            true,
        ),
        (
            "error",
            EventAction::Error,
            TaskState::Failed,
            Vars::new().with(crate::utils::consts::ACT_ERR_CODE, "E1"),
            false,
        ),
    ] {
        let workflow = one_step(
            &format!("pack-irq-{id}"),
            USES_IRQ,
            Vars::new().with("key", "act1"),
        );
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let created = fixture.wait_created("s1").await;
        fixture
            .act(&created.pid, &created.tid, action, options)
            .await
            .unwrap();
        fixture.wait_state("s1", state).await;
        if completed {
            fixture.wait_completed().await;
        } else {
            fixture.wait_error().await;
        }
        fixture.close().await;
    }
}
