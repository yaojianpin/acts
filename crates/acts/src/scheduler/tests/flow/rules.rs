// ---------------------------------------------------------------------------
// Step `rules` — custom input validations
// ---------------------------------------------------------------------------

use serial_test::serial;

use crate::{
    MessageState, Vars, Workflow, scheduler::core::model::task_graph::TaskState,
    utils::test::USES_IRQ,
};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

/// A triggered `error` rule fails the act at start, before it ever announces
/// itself to a client, with the rule's own message and the
/// `validation_failed` code.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_fail_the_act_before_it_announces() {
    let workflow = Workflow::new()
        .with_id("flow-rules-fail")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("amount", 0)
                .with_validation(crate::ValidationRule {
                    r#if: "amount <= 0".into(),
                    message: Some("amount must be positive".into()),
                    action: crate::event::EventAction::Error,
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Failed).await;
    assert!(
        fixture
            .message("s1", crate::event::MessageState::Created)
            .is_none(),
        "the act must fail before it announces itself"
    );
    let error = fixture.wait_error().await;
    assert!(
        error
            .outputs
            .to_string()
            .contains("amount must be positive")
            || format!("{error:?}").contains("amount must be positive"),
        "the failure carries the rule's message: {error:?}"
    );
    fixture.close().await;
}

/// No rule fires: the conditions read the resolved params by name and the
/// scope's variables, and all of them are false, so the act starts normally.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_pass_on_the_resolved_params_and_scope() {
    let workflow = Workflow::new()
        .with_id("flow-rules-pass")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1").with("amount", 5))
                .with_var("limit", 10)
                .with_validation(crate::ValidationRule {
                    r#if: "amount <= 0".into(),
                    message: None,
                    action: crate::event::EventAction::Error,
                })
                .with_validation(crate::ValidationRule {
                    r#if: "amount > limit".into(),
                    message: None,
                    action: crate::event::EventAction::Error,
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let act = fixture.wait_created("s1").await;

    fixture.next(&pid, &act.tid).await.unwrap();

    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// One post, one approver (the same person holds the manager and director posts). The
/// director approval's rule compares the previous approval's `approver`
/// (`$parent`) with the root's `director`; when they are the same person the
/// condition holds and its `skip` action drops that approval — no Created
/// message, the flow continues to the final step.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_skip_a_concurrent_post_approval() {
    let workflow = Workflow::new()
        .with_id("flow-rules-concurrent-post")
        .with_var("director", "u1")
        .with_step(|step| {
            step.with_id("manager_approval")
                .with_uses(USES_IRQ, Vars::new().with("key", "manager"))
                .with_var("approver", "u1")
        })
        .with_step(|step| {
            step.with_id("director_approval")
                .with_uses(USES_IRQ, Vars::new().with("key", "director"))
                .with_var("approver", "u1")
                .with_validation(crate::ValidationRule {
                    r#if: "$parent.approver == $root.director".into(),
                    message: Some("the director post is held by the manager".into()),
                    action: crate::event::EventAction::Skip,
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;

    let manager = fixture.wait_created("manager_approval").await;
    fixture.next(&pid, &manager.tid).await.unwrap();

    fixture
        .wait_state("director_approval", TaskState::Skipped)
        .await;
    assert!(
        fixture
            .message("director_approval", MessageState::Created)
            .is_none(),
        "a skipped approval must not reach a client"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// An auto-approval rule: the amount is under the approval limit, so the rule's
/// condition holds and its `next` action approves the act automatically — no
/// client decision and no manual `next` call.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_auto_approve_when_the_condition_holds() {
    let workflow = Workflow::new()
        .with_id("flow-rules-auto-approve")
        .with_var("amount", 100)
        .with_var("limit", 1000)
        .with_step(|step| {
            step.with_id("approval")
                .with_uses(USES_IRQ, Vars::new().with("key", "approval"))
                .with_validation(crate::ValidationRule {
                    r#if: "$root.amount <= $root.limit".into(),
                    message: None,
                    action: crate::event::EventAction::Next,
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("approval", TaskState::Completed).await;
    assert!(
        fixture.message("approval", MessageState::Created).is_none(),
        "an auto-approved act must not ask a client"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}
