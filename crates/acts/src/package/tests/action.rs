//! `acts.core.action`: an act issues a client action against the step it
//! belongs to, so the step ends in exactly the state that action produces
//! (`pack_action_*`).

use serial_test::serial;

use crate::{Vars, event::EventAction, scheduler::TaskState};

use super::{FlowFixture, USES_ACTION, one_step};

fn action_workflow(id: &str, action: EventAction, options: Vars) -> crate::model::Workflow {
    one_step(
        id,
        USES_ACTION,
        Vars::new()
            .with("action", action.as_ref())
            .with("options", options.to_value()),
    )
}

/// A step's action act completes the step before it (the package runs while
/// the step is waiting), and the options become the step's data.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_submit_marks_the_step_submitted_with_its_options() {
    let workflow = action_workflow(
        "pack-action-submit",
        EventAction::Submit,
        Vars::new().with("a", 5),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Submitted).await;
    assert_eq!(
        fixture.row("s1").await.data().get::<i32>("a"),
        Some(5),
        "the action options are written into the step's data"
    );
    fixture.wait_completed().await;
    fixture.close().await;
}

/// Each action lands its own state: next completes, skip skips, abort aborts
/// and error fails the step/run (`pack_action_*_on_step`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_each_decisions_state_is_the_steps_state() {
    for (id, action, state, options, completed) in [
        (
            "next",
            EventAction::Next,
            TaskState::Completed,
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
        let workflow = action_workflow(&format!("pack-action-{id}"), action, options);
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        fixture.wait_state("s1", state).await;
        if completed {
            fixture.wait_completed().await;
        } else {
            fixture.wait_error().await;
        }
        fixture.close().await;
    }
}

/// An action name the package's schema does not know fails the act instead of
/// silently doing nothing (the legacy `pack_action_not_exist`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_action_unknown_action_errors() {
    let mut workflow = one_step("pack-action-unknown", USES_ACTION, Vars::new());
    workflow.steps[0].params = serde_json::json!({ "action": "not_exist" });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_error().await;
    fixture.close().await;
}
